//! Controller behavior without a broker: feed request-ID idempotency, restart
//! recovery to uncertain, device report correlation, and the bounded,
//! filtered status the API exposes.

mod common;

use chrono::{Duration, Utc};
use petlibre_local::protocol::Channel;
use petlibre_local::{
    controller::{Controller, FeedInput, FeedPhase, Rejection},
    protocol::Message,
};
use serde_json::json;
use uuid::Uuid;

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
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();

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
    assert!(c.status.feeds.is_empty());

    // Observations cannot manufacture an outgoing command or imply a boot.
    assert!(
        petlibre_local::protocol::reply(
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
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();

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
fn retry_does_not_emit_another_command_and_restart_is_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let config = common::config();
    let mut c = Controller::load(config.clone(), path.clone()).unwrap();
    ready(&mut c);

    let id = Uuid::new_v4();
    assert!(
        c.feed(
            &FeedInput {
                request_id: id,
                portions: 1
            },
            Utc::now()
        )
        .unwrap()
        .1
        .is_some()
    );

    assert!(
        c.feed(
            &FeedInput {
                request_id: id,
                portions: 1
            },
            Utc::now()
        )
        .unwrap()
        .1
        .is_none()
    );

    assert!(matches!(
        c.feed(
            &FeedInput {
                request_id: id,
                portions: 2
            },
            Utc::now()
        ),
        Err(Rejection::Conflict)
    ));

    drop(c);
    let mut restarted = Controller::load(config, path).unwrap();
    ready(&mut restarted);
    let (record, command) = restarted
        .feed(
            &FeedInput {
                request_id: id,
                portions: 1,
            },
            Utc::now(),
        )
        .unwrap();
    assert_eq!(record.phase, FeedPhase::Uncertain);
    assert!(command.is_none());

    assert!(matches!(
        restarted.feed(
            &FeedInput {
                request_id: Uuid::new_v4(),
                portions: 1
            },
            Utc::now() + Duration::seconds(91)
        ),
        Err(Rejection::Offline)
    ));

    ready(&mut restarted);
    assert!(matches!(
        restarted.feed(
            &FeedInput {
                request_id: Uuid::new_v4(),
                portions: 1
            },
            Utc::now()
        ),
        Err(Rejection::Busy)
    ));
}

#[test]
fn acceptance_is_not_completion_and_wrong_topic_cannot_acknowledge_a_feed() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c);
    let id = Uuid::new_v4();

    c.feed(
        &FeedInput {
            request_id: id,
            portions: 1,
        },
        Utc::now(),
    )
    .unwrap();

    observe(
        &mut c,
        "event",
        json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":id,"code":0}),
    );
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Dispatching);

    observe(
        &mut c,
        "service",
        json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":id}),
    );
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Dispatching);

    observe(
        &mut c,
        "service",
        json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":id,"code":0}),
    );
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Accepted);

    observe(
        &mut c,
        "event",
        json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":id,"type":2,"execStep":"GRAIN_START","expectGrainNum":1,"actualGrainNum":0,"finished":false}),
    );
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Started);

    observe(
        &mut c,
        "event",
        json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":id,"type":2,"execStep":"GRAIN_END","expectGrainNum":1,"actualGrainNum":1,"finished":true}),
    );
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Completed);
}

#[test]
fn unrelated_grain_reports_cannot_advance_a_feed_with_matching_portion_counts() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c);

    let id = Uuid::new_v4();
    c.feed(
        &FeedInput {
            request_id: id,
            portions: 1,
        },
        Utc::now(),
    )
    .unwrap();

    observe(
        &mut c,
        "service",
        json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":id,"code":0}),
    );

    let grain = |report_id: Option<serde_json::Value>, step: &str| {
        let end = step == "GRAIN_END";
        let mut value = json!({"cmd":"GRAIN_OUTPUT_EVENT","type":2,"execStep":step,
            "expectGrainNum":1,"actualGrainNum":u8::from(end),"finished":end});
        if let Some(report_id) = report_id {
            value["msgId"] = report_id;
        }
        value
    };

    let unrelated = [
        Some(json!(Uuid::new_v4())),
        Some(json!("not-a-request-uuid")),
        Some(json!("")),
        Some(serde_json::Value::Null),
        None,
    ];

    for report_id in &unrelated {
        for step in ["GRAIN_START", "GRAIN_END"] {
            observe(&mut c, "event", grain(report_id.clone(), step));
            assert_eq!(c.status.feeds[0].phase, FeedPhase::Accepted);
            assert!(c.status.feeds[0].start_event_id.is_none());
            assert!(c.status.feeds[0].completion_event_id.is_none());
        }
    }

    observe(&mut c, "event", grain(Some(json!(id)), "GRAIN_START"));
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Started);

    for report_id in unrelated {
        observe(&mut c, "event", grain(report_id, "GRAIN_END"));
        assert_eq!(c.status.feeds[0].phase, FeedPhase::Started);
        assert!(c.status.feeds[0].completion_event_id.is_none());
    }

    observe(&mut c, "event", grain(Some(json!(id)), "GRAIN_END"));
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Completed);
}

#[test]
fn stock_feed_reuses_one_message_id_for_start_and_end_without_replaying() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let config = common::config();
    let mut c = Controller::load(config.clone(), path.clone()).unwrap();
    ready(&mut c);

    let id = Uuid::new_v4();
    c.feed(
        &FeedInput {
            request_id: id,
            portions: 1,
        },
        Utc::now(),
    )
    .unwrap();

    observe(
        &mut c,
        "service",
        json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":id,"code":0}),
    );

    let start = json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":id,"type":2,"execStep":"GRAIN_START","expectGrainNum":1,"actualGrainNum":0,"finished":false});
    let end = json!({"cmd":"GRAIN_OUTPUT_EVENT","msgId":id,"type":2,"execStep":"GRAIN_END","expectGrainNum":1,"actualGrainNum":1,"finished":true});

    // A reordered end cannot complete a feed that has not started.
    observe(&mut c, "event", end.clone());
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Accepted);

    observe(&mut c, "event", start.clone());
    observe(&mut c, "event", start.clone());
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Started);

    observe(&mut c, "event", end.clone());
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Completed);

    let completed_at = c.status.feeds[0].updated_at;
    observe(&mut c, "event", end.clone());
    observe(&mut c, "event", start.clone());
    assert_eq!(c.status.feeds[0].updated_at, completed_at);
    assert_eq!(c.status.feeds[0].start_event_id, Some(id.to_string()));
    assert_eq!(c.status.feeds[0].completion_event_id, Some(id.to_string()));

    drop(c);
    let mut c = Controller::load(config, path).unwrap();
    ready(&mut c);
    let (record, command) = c
        .feed(
            &FeedInput {
                request_id: id,
                portions: 1,
            },
            Utc::now(),
        )
        .unwrap();
    assert_eq!(record.phase, FeedPhase::Completed);
    assert!(command.is_none());

    // Delayed copies of the prior operation cannot advance a later operation.
    let later = Utc::now() + Duration::seconds(61);
    c.status.last_seen = Some(later);
    let next = Uuid::new_v4();
    c.feed(
        &FeedInput {
            request_id: next,
            portions: 1,
        },
        later,
    )
    .unwrap();

    observe(
        &mut c,
        "service",
        json!({"cmd":"MANUAL_FEEDING_SERVICE","msgId":next,"code":0}),
    );

    observe(&mut c, "event", start);
    observe(&mut c, "event", end);
    assert_eq!(c.status.feeds[0].request_id, next);
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Accepted);
}

#[test]
fn offline_invalid_amount_and_failed_persistence_cannot_send_a_feed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();

    assert!(matches!(
        c.feed(
            &FeedInput {
                request_id: Uuid::new_v4(),
                portions: 1
            },
            Utc::now()
        ),
        Err(Rejection::Offline)
    ));

    ready(&mut c);
    assert!(matches!(
        c.feed(
            &FeedInput {
                request_id: Uuid::new_v4(),
                portions: 0
            },
            Utc::now()
        ),
        Err(Rejection::InvalidPortions)
    ));

    // Replace the parent directory with a regular file to force a real I/O error.
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(path.with_extension("lock")).unwrap();
    std::fs::remove_dir(path.parent().unwrap()).unwrap();
    std::fs::write(path.parent().unwrap(), "not a directory").unwrap();

    let id = Uuid::new_v4();
    assert!(matches!(
        c.feed(
            &FeedInput {
                request_id: id,
                portions: 1
            },
            Utc::now()
        ),
        Err(Rejection::Storage)
    ));

    assert!(matches!(
        c.feed(
            &FeedInput {
                request_id: id,
                portions: 1
            },
            Utc::now()
        ),
        Err(Rejection::Storage)
    ));
}

#[test]
fn timeout_is_uncertain_and_private_attributes_are_not_exposed() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c);

    observe(
        &mut c,
        "event",
        json!({"cmd":"ATTR_PUSH_EVENT","wifiSsid":"secret-network","password":"secret","volume":50}),
    );
    assert!(!c.status.attributes.contains_key("wifiSsid"));
    assert!(!c.status.attributes.contains_key("password"));
    assert_eq!(c.status.attributes["volume"], 50);

    let now = Utc::now();
    c.feed(
        &FeedInput {
            request_id: Uuid::new_v4(),
            portions: 1,
        },
        now,
    )
    .unwrap();

    c.expire(now + Duration::seconds(91)).unwrap();
    assert_eq!(c.status.feeds[0].phase, FeedPhase::Uncertain);
    assert!(!c.status.device_online);
}

#[test]
fn concurrent_controllers_cannot_share_a_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let first = Controller::load(common::config(), path.clone()).unwrap();
    assert!(Controller::load(common::config(), path.clone()).is_err());
    drop(first);
    assert!(Controller::load(common::config(), path).is_ok());
}
