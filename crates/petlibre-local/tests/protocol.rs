//! Stock topic parsing and replies: only this device's `/post` topics parse,
//! no reply carries a feed command, and clock calibration is withheld while
//! imported plans exist.

mod common;

use chrono::Utc;
use petlibre_local::protocol;
use serde_json::json;

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

    let reply = protocol::reply(&config, &channel, &msg, Utc::now()).unwrap();
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
        assert_eq!(channel, "ota");
        assert_eq!(msg.cmd, cmd);
        assert!(protocol::reply(&config, &channel, &msg, Utc::now()).is_none());
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

    assert_eq!(channel, "config");
    assert!(protocol::reply(&config, &channel, &msg, Utc::now()).is_none());
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
    config.plans = vec![json!({"id":"existing-plan"})];
    let (_, msg) = protocol::parse(
        &config,
        &config.topic("event/post"),
        br#"{"cmd":"GET_FEEDING_PLAN_EVENT","msgId":"p"}"#,
    )
    .unwrap();

    let reply = protocol::reply(&config, "event", &msg, Utc::now()).unwrap();
    assert_eq!(reply.payload["plans"], json!(config.plans));

    let (_, msg) = protocol::parse(
        &config,
        &config.topic("ntp/post"),
        br#"{"cmd":"NTP","ts":0}"#,
    )
    .unwrap();
    assert_eq!(
        protocol::reply(&config, "ntp", &msg, Utc::now())
            .unwrap()
            .payload["calibrationTag"],
        false
    );
}
