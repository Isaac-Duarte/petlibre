//! Golden bytes of every reply and command the server sends the stock
//! firmware. The device parses these with its own JSON reader, so the keys,
//! values and (sorted) key order are pinned exactly; only the random
//! `linkId` is masked.

mod common;

use chrono::{TimeZone, Utc};
use wellfed_local::{
    controller::{Controller, FeedInput},
    ota_trial::{self, Image},
    probe,
    protocol::{self, Reply},
};
use serde_json::{Value, json};
use uuid::Uuid;

const ID: &str = "6c8e4f8a-0d43-4c4b-9d5e-1f2a3b4c5d6e";

fn bytes(reply: &Reply) -> String {
    let mut payload = reply.payload.clone();
    if payload.get("linkId").is_some_and(Value::is_string) {
        payload["linkId"] = json!("LINK");
    }
    serde_json::to_string(&payload).unwrap()
}

fn answer(config: &wellfed_local::config::Config, channel: &str, message: Value) -> String {
    let (channel, message) = protocol::parse(
        config,
        &config.topic(&format!("{channel}/post")),
        &serde_json::to_vec(&message).unwrap(),
    )
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 17, 21, 17, 41).unwrap();
    bytes(&protocol::reply(config, channel, &message, now).unwrap())
}

#[test]
fn acknowledgements() {
    let mut config = common::config();

    assert_eq!(
        answer(
            &config,
            "event",
            json!({"cmd":"DEVICE_START_EVENT","msgId":"m1","ts":5})
        ),
        r#"{"cmd":"DEVICE_START_EVENT","code":0,"linkId":"LINK","msgId":"m1","ts":1792271861000}"#
    );
    assert_eq!(
        answer(&config, "event", json!({"cmd":"ERROR_EVENT"})),
        r#"{"cmd":"ERROR_EVENT","code":0,"linkId":"LINK","ts":1792271861000}"#
    );
    assert_eq!(
        answer(
            &config,
            "event",
            json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":"m2","execStep":"GRAIN_END"})
        ),
        r#"{"cmd":"GRAIN_OUTPUT_EVENT","code":0,"execStep":"GRAIN_END","linkId":"LINK","msgId":"m2","ts":1792271861000}"#
    );
    // The step is echoed as the device sent it, whatever its type.
    assert_eq!(
        answer(
            &config,
            "event",
            json!({"cmd":"GRAIN_OUTPUT_EVENT","execStep":{"b":1,"a":[2]}})
        ),
        r#"{"cmd":"GRAIN_OUTPUT_EVENT","code":0,"execStep":{"a":[2],"b":1},"linkId":"LINK","ts":1792271861000}"#
    );

    config.plans = vec![json!({"z":1,"id":"p"}), json!(3)];
    assert_eq!(
        answer(
            &config,
            "event",
            json!({"cmd":"GET_FEEDING_PLAN_EVENT","msgId":"m3"})
        ),
        r#"{"cmd":"GET_FEEDING_PLAN_EVENT","code":0,"linkId":"LINK","msgId":"m3","plans":[{"id":"p","z":1},3],"ts":1792271861000}"#
    );
}

#[test]
fn clock() {
    let mut config = common::config();
    assert_eq!(
        answer(&config, "ntp", json!({"cmd":"NTP","msgId":"n"})),
        r#"{"calibrationTag":true,"cmd":"NTP","code":0,"linkId":"LINK","nextDSTOffsetSeconds":-25200,"nextDSTTransitionTs":1793520000000,"secondNextDSTOffsetSeconds":-21600,"secondNextDSTTransitionTs":1805014800000,"timezone":-6.0,"timezoneOffsetSeconds":-21600,"ts":1792271861000}"#
    );

    config.plans = vec![json!({})];
    config.timezone = chrono_tz::UTC;
    assert_eq!(
        answer(&config, "ntp", json!({"cmd":"NTP"})),
        r#"{"calibrationTag":false,"cmd":"NTP","code":0,"linkId":"LINK","nextDSTOffsetSeconds":0,"nextDSTTransitionTs":0,"secondNextDSTOffsetSeconds":0,"secondNextDSTTransitionTs":0,"timezone":0.0,"timezoneOffsetSeconds":0,"ts":1792271861000}"#
    );
}

#[test]
fn binding_is_replayed_with_fresh_ids() {
    let mut config = common::config();
    let Value::Object(binding) = json!({"cmd":"BINDING","code":0,"z":"kept","ts":1}) else {
        unreachable!()
    };
    config.binding_response = Some(binding);

    assert_eq!(
        answer(&config, "system", json!({"cmd":"BINDING","msgId":"b"})),
        r#"{"cmd":"BINDING","code":0,"linkId":"LINK","msgId":"b","ts":1792271861000,"z":"kept"}"#
    );
    assert_eq!(
        answer(&config, "system", json!({"cmd":"BINDING"})),
        r#"{"cmd":"BINDING","code":0,"linkId":"LINK","msgId":null,"ts":1792271861000,"z":"kept"}"#
    );
}

#[test]
fn commands() {
    let config = common::config();
    let id = Uuid::parse_str(ID).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 17, 21, 17, 41).unwrap();

    let query = protocol::service(&config, "ATTR_GET_SERVICE", id, now);
    assert_eq!(query.topic, config.topic("service/sub"));
    assert_eq!(
        bytes(&query),
        format!(r#"{{"cmd":"ATTR_GET_SERVICE","msgId":"{ID}","ts":1792271861000}}"#)
    );

    let probe = probe::command(&config, id, now);
    assert_eq!(probe.topic, config.topic("config/sub"));
    assert_eq!(
        bytes(&probe),
        format!(r#"{{"cmd":"GET_CONFIG","msgId":"{ID}","ts":1792271861000}}"#)
    );

    let trial = ota_trial::Record {
        sequence: 1,
        input: ota_trial::Input {
            request_id: id,
            image: Image::CProbe,
            server_token: Uuid::nil(),
        },
        created_at: now,
        updated_at: now,
        phase: ota_trial::Phase::Submitted,
        download_reported: false,
        resolution_note: None,
    };
    let upgrade = trial.command(&config);
    assert_eq!(upgrade.topic, config.topic("service/sub"));
    assert_eq!(
        bytes(&upgrade),
        format!(
            r#"{{"cmd":"OTA_UPGRADE","msgId":"{ID}","sha256":"{}","targetPid":"PLAF103","targetSoftwareVersion":"1.1.17","ts":1792271861000,"upgradeType":1,"url":"http://127.0.0.1:8088/ota/00000000-0000-0000-0000-000000000000/probe.bin"}}"#,
            wellfed_local::ota_image::C_PROBE_SHA256
        )
    );
}

#[test]
fn feed_command() {
    let dir = tempfile::tempdir().unwrap();
    let mut controller =
        Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    let now = Utc::now();
    controller.status.broker_connected = true;
    controller.status.last_seen = Some(now);

    let id = Uuid::parse_str(ID).unwrap();
    let (_, command) = controller
        .feed(
            FeedInput {
                request_id: id,
                portions: 1,
            },
            now,
        )
        .unwrap();
    let command = command.unwrap();

    assert_eq!(command.topic, controller.config.topic("service/sub"));
    assert_eq!(
        bytes(&command),
        format!(
            r#"{{"cmd":"MANUAL_FEEDING_SERVICE","grainNum":1,"msgId":"{ID}","ts":{}}}"#,
            now.timestamp_millis()
        )
    );
}
