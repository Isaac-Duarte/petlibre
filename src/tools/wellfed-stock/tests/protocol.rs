//! Stock topic parsing and replies: only this device's `/post` topics parse,
//! no reply carries a feed command, clock calibration is withheld while
//! plans may exist, and no reply carries plans that were not captured.

mod common;

use chrono::Utc;
use serde_json::json;
use wellfed_stock::protocol;
use wellfed_stock::protocol::Channel;

#[test]
fn messages_from_other_devices_and_retained_topic_shapes_are_rejected() {
    let config = common::config();
    for topic in [
        "dl/PLAF103/ANOTHER/device/event/post",
        "dl/PLAF103/AF_TEST_ONLY/device/event/sub",
        "dl/PLAF103/AF_TEST_ONLY/device/unknown/post",
    ] {
        assert!(protocol::parse(&config, topic, br#"{"cmd":"ATTR_PUSH_EVENT"}"#).is_err());
    }

    assert!(protocol::parse(&config, &config.topic("event/post"), b"[]").is_err());
    assert!(protocol::parse(&config, &config.topic("event/post"), &vec![b'x'; 65_537]).is_err());
}

#[test]
fn acknowledgements_preserve_event_id_and_step_without_sending_a_feed() {
    let config = common::config();
    let input = json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":"event42","execStep":"GRAIN_END"});
    let (channel, msg) = protocol::parse(
        &config,
        &config.topic("event/post"),
        &serde_json::to_vec(&input).unwrap(),
    )
    .unwrap();

    let reply = protocol::reply(&config, channel, &msg, Utc::now()).unwrap();
    assert_eq!(reply.topic, config.topic("event/sub"));
    assert_eq!(reply.payload["msgId"], "event42");
    assert_eq!(reply.payload["execStep"], "GRAIN_END");
    assert_eq!(reply.payload["code"], 0);
    assert!(reply.payload.get("grainNum").is_none());
}

#[test]
fn stock_ota_report_topic_is_parsed_without_sending_an_acknowledgement_or_command() {
    let config = common::config();
    for cmd in ["OTA_UPGRADE", "OTA_PROGRESS", "OTA_INFORM"] {
        let payload =
            serde_json::to_vec(&json!({"cmd":cmd,"msgId":"trial","progress":40})).unwrap();
        let (channel, msg) = protocol::parse(&config, &config.topic("ota/post"), &payload).unwrap();
        assert_eq!(channel, Channel::Ota);
        assert_eq!(msg.cmd, cmd);
        assert!(protocol::reply(&config, channel, &msg, Utc::now()).is_none());
        assert!(protocol::parse(&config, "dl/PLAF103/ANOTHER/device/ota/post", &payload).is_err());
    }
}

#[test]
fn configuration_reply_uses_its_own_channel_without_an_automatic_response() {
    let config = common::config();
    let (channel, msg) = protocol::parse(
        &config,
        &config.topic("config/post"),
        br#"{"cmd":"GET_CONFIG","msgId":"device-generated"}"#,
    )
    .unwrap();

    assert_eq!(channel, Channel::Config);
    assert!(protocol::reply(&config, channel, &msg, Utc::now()).is_none());
    assert!(
        protocol::parse(
            &config,
            "dl/PLAF103/ANOTHER/device/config/post",
            br#"{"cmd":"GET_CONFIG"}"#
        )
        .is_err()
    );
}

#[test]
fn plan_requests_return_imported_snapshot_and_clock_changes_are_held_for_nonempty_plans() {
    let mut config = common::config();
    config.plans = Some(vec![json!({"id":"existing-plan"})]);
    let (_, msg) = protocol::parse(
        &config,
        &config.topic("event/post"),
        br#"{"cmd":"GET_FEEDING_PLAN_EVENT","msgId":"p"}"#,
    )
    .unwrap();

    let reply = protocol::reply(&config, Channel::Event, &msg, Utc::now()).unwrap();
    assert_eq!(reply.payload["plans"], json!(config.plans.clone().unwrap()));

    let (_, msg) = protocol::parse(
        &config,
        &config.topic("ntp/post"),
        br#"{"cmd":"NTP","ts":0}"#,
    )
    .unwrap();
    assert_eq!(
        protocol::reply(&config, Channel::Ntp, &msg, Utc::now())
            .unwrap()
            .payload["calibrationTag"],
        false
    );
}

/// Stock erases and rewrites its plan table on any message with a `plans`
/// key. With no captured snapshot (an installer's or enrolled config), the
/// plan request goes unanswered, no reply carries the key, and the clock is
/// not calibrated; a captured snapshot, empty or not, is answered as
/// captured.
#[test]
fn plans_are_sent_only_as_captured() {
    let mut config = common::config();
    config.plans = None;
    let ask = |config: &wellfed_stock::config::Config, channel: Channel, topic: &str, cmd: &str| {
        let body = json!({"cmd": cmd, "msgId": "m", "ts": 0, "execStep": "GRAIN_END"});
        let (parsed, msg) = protocol::parse(
            config,
            &config.topic(topic),
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap();
        assert_eq!(parsed, channel);
        protocol::reply(config, channel, &msg, Utc::now())
    };

    assert!(
        ask(
            &config,
            Channel::Event,
            "event/post",
            "GET_FEEDING_PLAN_EVENT"
        )
        .is_none()
    );
    for (channel, topic, cmd) in [
        (Channel::Ntp, "ntp/post", "NTP"),
        (Channel::Event, "event/post", "GRAIN_OUTPUT_EVENT"),
        (Channel::Event, "event/post", "DEVICE_START_EVENT"),
        (Channel::Event, "event/post", "ATTR_PUSH_EVENT"),
        (Channel::Event, "event/post", "DEVICE_LOG_REPORT_EVENT"),
        (Channel::Event, "event/post", "ERROR_EVENT"),
    ] {
        let reply = ask(&config, channel, topic, cmd).expect("acknowledged");
        assert!(reply.payload.get("plans").is_none(), "{cmd}");
    }
    let ntp = ask(&config, Channel::Ntp, "ntp/post", "NTP").unwrap();
    assert_eq!(ntp.payload["calibrationTag"], false);

    for captured in [vec![], vec![json!({"id": "p"})]] {
        config.plans = Some(captured.clone());
        let reply = ask(
            &config,
            Channel::Event,
            "event/post",
            "GET_FEEDING_PLAN_EVENT",
        )
        .unwrap();
        assert_eq!(reply.payload["plans"], json!(captured));
    }
}

/// The bench seeds stock's table through a captured config whose `plans`
/// is a test table: every plan carries a far-future `skipEndTime`, so stock
/// stores it but never runs it. The table goes out exactly as written,
/// field for field (stock's reply handler reads `planId`, `executionTime`,
/// `grainNum`, `repeatDay`, `enableAudio`, `audioTimes`, `syncTime` and
/// `skipEndTime`), with the clock held. With `plans` then removed from the
/// file, the request goes unanswered again, so the install that follows
/// finds the seeded table intact.
#[test]
fn a_seeded_table_replays_as_written_and_removing_it_leaves_stock_s_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seed.json");
    let seed = json!([
        {"planId": 9001, "executionTime": "07:30", "grainNum": 2,
         "repeatDay": [1, 2, 3, 4, 5, 6, 7], "enableAudio": false, "audioTimes": 0,
         "syncTime": 1_792_271_861_000_i64, "skipEndTime": 4_102_444_800_000_i64},
        {"planId": 9002, "executionTime": "18:05", "grainNum": 3, "repeatDay": [6, 7],
         "enableAudio": true, "audioTimes": 2,
         "syncTime": 1_792_271_861_000_i64, "skipEndTime": 4_102_444_800_000_i64},
    ]);
    let mut file = serde_json::to_value(common::config()).unwrap();
    file["plans"] = seed.clone();
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

    let config = wellfed_stock::config::load(&path).unwrap();
    let ask = |config: &wellfed_stock::config::Config, topic: &str, cmd: &str| {
        let body = serde_json::to_vec(&json!({"cmd": cmd, "msgId": "m", "ts": 0})).unwrap();
        let (channel, msg) = protocol::parse(config, &config.topic(topic), &body).unwrap();
        protocol::reply(config, channel, &msg, Utc::now())
    };
    let reply = ask(&config, "event/post", "GET_FEEDING_PLAN_EVENT").unwrap();
    assert_eq!(reply.payload["plans"], seed);
    assert_eq!(
        serde_json::to_string(&reply.payload["plans"][0]).unwrap(),
        r#"{"audioTimes":0,"enableAudio":false,"executionTime":"07:30","grainNum":2,"planId":9001,"repeatDay":[1,2,3,4,5,6,7],"skipEndTime":4102444800000,"syncTime":1792271861000}"#
    );
    assert_eq!(
        ask(&config, "ntp/post", "NTP").unwrap().payload["calibrationTag"],
        false
    );

    file.as_object_mut().unwrap().remove("plans");
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let config = wellfed_stock::config::load(&path).unwrap();
    assert_eq!(config.plans, None);
    assert!(ask(&config, "event/post", "GET_FEEDING_PLAN_EVENT").is_none());
}
