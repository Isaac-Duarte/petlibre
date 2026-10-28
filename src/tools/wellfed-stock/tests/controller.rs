//! Controller behavior without a broker: the bounded, filtered status the
//! API exposes, the ledger's lock, and the refusal of a retired ledger.
//! Install trials are `ota_trial.rs`.

mod common;

use chrono::{Duration, Utc};
use common::Controller;
use serde_json::json;
use wellfed_stock::protocol::Channel;
use wellfed_stock::{
    controller::{LEDGER_FILE, RETIRED_LEDGER_FILE},
    protocol::Message,
};

fn observe(controller: &mut Controller, channel: &str, value: serde_json::Value) {
    let message: Message = serde_json::from_value(value).unwrap();
    controller
        .observe(Channel::from_name(channel).unwrap(), &message, Utc::now())
        .unwrap();
}

fn ready(controller: &mut Controller) {
    controller.status.broker_connected = true;
    observe(
        controller,
        "heart",
        json!({"cmd":"HEARTBEAT","ts":Utc::now().timestamp_millis()}),
    );
}

#[test]
fn ota_reports_keep_distinct_phases_with_shared_ids_and_bound_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = common::load(dir.path().join("ledger.json")).unwrap();

    for progress in 0..=20 {
        observe(
            &mut c,
            "system",
            json!({"cmd":"OTA_PROGRESS","msgId":"same-update","progress":progress}),
        );
    }

    assert_eq!(c.status.ota_reports.len(), 16);
    assert_eq!(c.status.ota_reports[0].progress, Some(5));

    observe(
        &mut c,
        "system",
        json!({"cmd":"OTA_UPGRADE","msgId":"same-update","phase":"finalize","code":0}),
    );
    observe(
        &mut c,
        "system",
        json!({"cmd":"OTA_INFORM","msgId":"same-update","state":6,"errCode":4001}),
    );

    let reports = &c.status.ota_reports;
    assert_eq!(reports.len(), 16);
    assert_eq!(reports[14].phase.as_deref(), Some("finalize"));
    assert_eq!(reports[15].state, Some(6));
    assert_eq!(reports[15].error_code, Some(4001));
    assert!(c.status.ota_trials.is_empty());

    // Observations cannot manufacture an outgoing command or imply a boot.
    assert!(
        wellfed_stock::protocol::reply(
            &c.config,
            Channel::System,
            &serde_json::from_value(json!({"cmd":"OTA_INFORM","state":5})).unwrap(),
            Utc::now()
        )
        .is_none()
    );
}

#[test]
fn ota_status_drops_unbounded_or_private_fields_and_survives_disconnect_as_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = common::load(dir.path().join("ledger.json")).unwrap();

    observe(
        &mut c,
        "system",
        json!({"cmd":"OTA_INFORM","msgId":"x".repeat(1000),"phase":"y".repeat(1000),
            "state":256,"progress":101,"errCode":-1,"code":0,
            "errorMsg":"private-url","speedTest":{"url":"private-url"},"password":"secret"}),
    );

    let report = &c.status.ota_reports[0];
    assert!(report.message_id.is_none() && report.phase.is_none());
    assert!(report.state.is_none() && report.progress.is_none() && report.error_code.is_none());
    let output = serde_json::to_string(report).unwrap();
    assert!(!output.contains("private-url") && !output.contains("secret"));
    assert!(report.received_at <= Utc::now());

    c.disconnected(Utc::now()).unwrap();
    assert!(!c.status.device_online);
    assert_eq!(c.status.ota_reports.len(), 1);
    observe(&mut c, "heart", json!({"cmd":"HEARTBEAT"}));
    assert_eq!(c.status.ota_reports.len(), 1);
}

#[test]
fn stale_telemetry_goes_offline_and_private_attributes_are_not_exposed() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = common::load(dir.path().join("ledger.json")).unwrap();
    ready(&mut c);

    observe(
        &mut c,
        "event",
        json!({"cmd":"ATTR_PUSH_EVENT","wifiSsid":"secret-network","password":"secret","volume":50}),
    );
    assert!(!c.status.attributes.contains_key("wifiSsid"));
    assert!(!c.status.attributes.contains_key("password"));
    assert_eq!(c.status.attributes["volume"], 50);

    assert!(c.status.device_online);
    c.expire(Utc::now() + Duration::seconds(91)).unwrap();
    assert!(!c.status.device_online);
}

#[test]
fn concurrent_controllers_cannot_share_a_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let first = common::load(path.clone()).unwrap();
    assert!(common::load(path.clone()).is_err());
    drop(first);
    assert!(common::load(path).is_ok());
}

#[test]
fn a_ledger_from_before_fw51_is_never_read_or_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let retired = dir.path().join(RETIRED_LEDGER_FILE);
    let old = br#"{"device_id":"test-device","feeds":{},"ota_trials":{},"ota_hold":true,"probe_queries":{}}"#;
    std::fs::write(&retired, old).unwrap();

    let refused = common::load(dir.path().join(LEDGER_FILE)).err().unwrap();
    assert!(refused.to_string().contains("archive it"));
    assert_eq!(std::fs::read(&retired).unwrap(), old);
    assert!(!dir.path().join(LEDGER_FILE).exists());

    // The same records under the new name are refused, not silently dropped.
    std::fs::rename(&retired, dir.path().join(LEDGER_FILE)).unwrap();
    assert!(common::load(dir.path().join(LEDGER_FILE)).is_err());
}
