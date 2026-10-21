mod common;

use chrono::{DateTime, Duration, Utc};
use petlibro_local::{
    controller::{Controller, Rejection},
    ota_image::{C_PROBE_SHA256, Lease, ORIGINAL_SHA256, Profile},
    ota_trial::{self, Image},
    probe::{self, Phase},
};
use serde_json::{Value, json};
use uuid::Uuid;

fn observe(c: &mut Controller, channel: &str, value: Value, at: DateTime<Utc>) {
    c.observe(channel, &serde_json::from_value(value).unwrap(), at)
        .unwrap();
}

fn ready(c: &mut Controller, version: &str, at: DateTime<Utc>) {
    c.connected(at);
    observe(
        c,
        "event",
        json!({"cmd":"DEVICE_START_EVENT", "softwareVersion":version,
        "powerType":1, "ts":at.timestamp_millis()}),
        at,
    );
}

fn lease(profile: Profile, at: DateTime<Utc>) -> Lease {
    Lease {
        token: Uuid::new_v4(),
        peer: "10.0.0.2".parse().unwrap(),
        expires_at: at + Duration::minutes(15),
        profile,
    }
}

fn installed(c: &mut Controller, at: DateTime<Utc>) -> Uuid {
    ready(c, "1.1.15", at);
    c.enable_ota_trial();
    let images = lease(Profile::CExecution, at);
    let input = ota_trial::Input {
        request_id: Uuid::new_v4(),
        image: Image::CProbe,
        server_token: images.token,
    };
    let command = c
        .ota_trial(input.clone(), Some(&images), at)
        .unwrap()
        .1
        .unwrap();
    assert_eq!(command.payload["sha256"], C_PROBE_SHA256);
    assert!(
        command.payload["url"]
            .as_str()
            .unwrap()
            .ends_with("/probe.bin")
    );
    ready(c, "1.1.17", at + Duration::milliseconds(1));
    assert!(c.status.ota_hold);
    input.request_id
}

fn reply(id: &str, at: DateTime<Utc>) -> Value {
    json!({"cmd":"GET_CONFIG", "msgId":id, "softwareVersion":"1.1.17",
        "localReProbe":probe::expected(id).unwrap(), "ts":at.timestamp_millis()})
}

#[test]
fn query_is_journaled_once_and_requires_two_fresh_distinct_computed_replies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();
    let at = Utc::now();
    let trial = installed(&mut c, at);
    let at = at + Duration::seconds(1);
    let input = probe::Input {
        request_id: Uuid::new_v4(),
    };
    let (record, command) = c.probe_query(input.clone(), at).unwrap();
    assert_eq!(record.trial_request_id, trial);
    assert_eq!(command.unwrap().payload["cmd"], "GET_CONFIG");
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        disk["probe_queries"][input.request_id.to_string()]["phase"],
        "submitted"
    );
    assert!(c.probe_query(input.clone(), at).unwrap().1.is_none());
    assert!(matches!(
        c.probe_query(
            probe::Input {
                request_id: Uuid::new_v4()
            },
            at
        ),
        Err(Rejection::InvalidProbe)
    ));

    let mut malformed = reply("device-generated-1", at);
    malformed["localReProbe"] = json!("c1-00000000");
    observe(&mut c, "config", malformed, at);
    observe(&mut c, "event", reply("device-generated-1", at), at);
    let mut stale = reply("device-generated-1", at);
    stale["ts"] = json!((at - Duration::seconds(11)).timestamp_millis());
    observe(&mut c, "config", stale, at);
    let mut wrong_version = reply("device-generated-1", at);
    wrong_version["softwareVersion"] = json!("1.1.15");
    observe(&mut c, "config", wrong_version, at);
    assert_eq!(c.status.probe_queries[0].phase, Phase::Submitted);
    let mut skewed = reply("device-generated-1", at);
    skewed["ts"] = json!((at - Duration::seconds(2)).timestamp_millis());
    observe(&mut c, "config", skewed, at);
    assert_eq!(c.status.probe_queries[0].phase, Phase::ProofReported);
    assert_ne!(
        c.status.probe_queries[0].proof.as_ref().unwrap().message_id,
        input.request_id.to_string()
    );

    let at = at + Duration::seconds(1);
    let second = probe::Input {
        request_id: Uuid::new_v4(),
    };
    c.probe_query(second, at).unwrap();
    observe(&mut c, "config", reply("device-generated-1", at), at);
    assert_eq!(c.status.probe_queries[0].phase, Phase::Submitted);
    observe(&mut c, "config", reply("device-generated-2", at), at);
    assert!(
        c.status
            .probe_queries
            .iter()
            .all(|q| q.phase == Phase::ProofReported)
    );
    assert!(c.status.ota_hold);
    assert_eq!(probe::expected("hello").as_deref(), Some("c1-4f9f2cab"));
    for invalid in ["", "\n", "é", &"x".repeat(48)] {
        assert!(probe::expected(invalid).is_none());
    }
}

#[test]
fn pending_query_is_not_replayed_after_restart_and_does_not_block_original_restore() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();
    let at = Utc::now();
    installed(&mut c, at);
    let input = probe::Input {
        request_id: Uuid::new_v4(),
    };
    c.probe_query(input.clone(), at + Duration::seconds(1))
        .unwrap();
    drop(c);
    let mut c = Controller::load(common::config(), path).unwrap();
    assert_eq!(c.status.probe_queries[0].phase, Phase::Uncertain);
    assert!(c.probe_query(input, at).unwrap().1.is_none());
    c.enable_ota_trial();
    let at = at + Duration::seconds(2);
    ready(&mut c, "1.1.17", at);
    assert!(matches!(
        c.probe_query(
            probe::Input {
                request_id: Uuid::new_v4()
            },
            at
        ),
        Err(Rejection::InvalidProbe)
    ));
    let images = lease(Profile::CExecution, at);
    let restore = ota_trial::Input {
        request_id: Uuid::new_v4(),
        image: Image::CProbeOriginal,
        server_token: images.token,
    };
    let command = c.ota_trial(restore, Some(&images), at).unwrap().1.unwrap();
    assert_eq!(command.payload["targetSoftwareVersion"], "1.1.15");
    assert_eq!(command.payload["sha256"], ORIGINAL_SHA256);
    ready(&mut c, "1.1.15", at + Duration::seconds(1));
    assert!(!c.status.ota_hold);
}

#[test]
fn profile_mismatch_and_unconfirmed_installation_cannot_execute_the_probe() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    let at = Utc::now();
    ready(&mut c, "1.1.15", at);
    let query = probe::Input {
        request_id: Uuid::new_v4(),
    };
    assert!(matches!(
        c.probe_query(query.clone(), at),
        Err(Rejection::OtaDisabled)
    ));
    c.enable_ota_trial();
    assert!(matches!(
        c.probe_query(query, at),
        Err(Rejection::InvalidProbe)
    ));
    let wrong = lease(Profile::StockVersion, at);
    let input = ota_trial::Input {
        request_id: Uuid::new_v4(),
        image: Image::CProbe,
        server_token: wrong.token,
    };
    assert!(matches!(
        c.ota_trial(input, Some(&wrong), at),
        Err(Rejection::ImageServer)
    ));
    let correct = lease(Profile::CExecution, at);
    ready(&mut c, "1.1.17", at);
    let restore = ota_trial::Input {
        request_id: Uuid::new_v4(),
        image: Image::CProbeOriginal,
        server_token: correct.token,
    };
    assert!(matches!(
        c.ota_trial(restore, Some(&correct), at),
        Err(Rejection::InvalidOta)
    ));
}

#[test]
fn lost_query_times_out_and_cannot_be_retriggered_by_a_late_reply() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    let at = Utc::now();
    installed(&mut c, at);
    let at = at + Duration::seconds(1);
    c.probe_query(
        probe::Input {
            request_id: Uuid::new_v4(),
        },
        at,
    )
    .unwrap();
    let later = at + Duration::seconds(60);
    c.expire(later).unwrap();
    observe(&mut c, "config", reply("late-reply", later), later);
    assert_eq!(c.status.probe_queries[0].phase, Phase::Uncertain);
    assert!(c.status.probe_queries[0].proof.is_none());
    assert!(c.status.ota_hold);
}
