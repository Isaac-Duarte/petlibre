//! OTA trial rules in the controller, without a broker: dispatch guards,
//! journal-before-publish with no replay, the persistent feeding hold, and
//! evidence-based resolution of uncertain trials.

mod common;

use chrono::{DateTime, Duration, Utc};
use wellfed_local::protocol::Channel;
use wellfed_local::{
    controller::{Controller, FeedInput, Rejection},
    ota_image::Lease,
    ota_trial::{self, Image, Phase, ResolveInput},
    protocol::{Message, Reply},
};
use serde_json::json;
use uuid::Uuid;

fn lease(token: Uuid, now: DateTime<Utc>) -> Lease {
    Lease {
        token,
        peer: "10.0.0.2".parse().unwrap(),
        expires_at: now + Duration::minutes(15),
        profile: wellfed_local::ota_image::Profile::StockVersion,
    }
}

/// Submit with a live image-server lease matching the request's token.
fn submit(
    c: &mut Controller,
    input: ota_trial::Input,
    now: DateTime<Utc>,
) -> Result<(ota_trial::Record, Option<Reply>), Rejection> {
    let lease = lease(input.server_token, now);
    c.ota_trial(input, Some(&lease), now)
}

fn observe(c: &mut Controller, channel: &str, value: serde_json::Value, now: DateTime<Utc>) {
    let message: Message = serde_json::from_value(value).unwrap();
    c.observe(Channel::from_name(channel).unwrap(), &message, now)
        .unwrap();
}

/// Simulate continued feeder presence; broker connectivity alone is insufficient.
fn keep_connected(c: &mut Controller, until: DateTime<Utc>) {
    let mut at = c.status.last_seen.unwrap() + Duration::seconds(30);
    while at <= until {
        observe(c, "heart", json!({"cmd":"HEARTBEAT"}), at);
        at += Duration::seconds(30);
    }
}

fn resolve(id: Uuid) -> ResolveInput {
    ResolveInput {
        request_id: id,
        note: "checked fresh version report and quiet period".to_owned(),
    }
}

fn ready(c: &mut Controller, version: &str) {
    c.status.broker_connected = true;
    c.observe(
        Channel::Event,
        &serde_json::from_value(json!({"cmd":"ATTR_PUSH_EVENT",
        "softwareVersion":version,"powerType":1}))
        .unwrap(),
        Utc::now(),
    )
    .unwrap();
}

fn input(image: Image) -> ota_trial::Input {
    ota_trial::Input {
        request_id: Uuid::new_v4(),
        image,
        server_token: Uuid::new_v4(),
    }
}

#[test]
fn default_disabled_and_guarded_versions_power_plans_and_active_feeds() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");

    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::OtaDisabled)
    ));

    c.enable_ota_trial();
    c.status.version_reported_at = None;
    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    c.status.version_reported_at = Some(Utc::now() - Duration::seconds(301));
    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    for version in ["1.1.14", "1.1.16", "unverified"] {
        ready(&mut c, version);
        assert!(matches!(
            submit(&mut c, input(Image::Test), Utc::now()),
            Err(Rejection::InvalidOta)
        ));
    }

    ready(&mut c, "1.1.15");
    c.status.attributes.insert("powerType".to_owned(), json!(2));
    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    c.status.attributes.insert("powerType".to_owned(), json!(1));
    c.config.plans.push(json!({"id":"plan"}));
    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    c.config.plans.clear();
    c.feed(
        &FeedInput {
            request_id: Uuid::new_v4(),
            portions: 1,
        },
        Utc::now(),
    )
    .unwrap();
    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::Busy)
    ));

    assert!(c.status.ota_trials.is_empty());
}

#[test]
fn journal_before_publish_duplicate_conflict_restart_and_timeout_never_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let request = input(Image::Test);
    let now = Utc::now();
    let (record, command) = submit(&mut c, request.clone(), now).unwrap();
    let command = command.unwrap();

    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        on_disk["ota_trials"][request.request_id.to_string()]["phase"],
        "submitted"
    );

    assert_eq!(command.payload["targetSoftwareVersion"], "1.1.16");
    assert_eq!(command.payload["upgradeType"], 1);
    assert_eq!(
        command.payload["sha256"],
        wellfed_local::ota_image::TEST_SHA256
    );
    assert!(
        command.payload["url"]
            .as_str()
            .unwrap()
            .ends_with("/test.bin")
    );
    assert!(
        !command
            .payload
            .to_string()
            .contains("MANUAL_FEEDING_SERVICE")
    );

    assert_eq!(record.phase, Phase::Submitted);
    assert!(submit(&mut c, request.clone(), now).unwrap().1.is_none());

    let mut conflicting = request.clone();
    conflicting.server_token = Uuid::new_v4();
    assert!(matches!(
        submit(&mut c, conflicting, now),
        Err(Rejection::Conflict)
    ));

    c.expire(now + Duration::seconds(600)).unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);

    drop(c);
    let mut c = Controller::load(common::config(), path).unwrap();
    assert!(c.status.ota_hold);
    assert!(!c.status.ota_trial_enabled);
    assert!(submit(&mut c, request, now).unwrap().1.is_none());
    assert!(matches!(
        c.feed(
            &FeedInput {
                request_id: Uuid::new_v4(),
                portions: 1
            },
            now
        ),
        Err(Rejection::OtaActive)
    ));
}

#[test]
fn progress_and_stale_startup_cannot_confirm_trial_and_only_original_version_clears_hold() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let request = input(Image::Test);
    let now = Utc::now();
    submit(&mut c, request.clone(), now).unwrap();

    for value in [
        json!({"cmd":"OTA_PROGRESS","msgId":request.request_id,"progress":100}),
        json!({"cmd":"DEVICE_START_EVENT","ts":now.timestamp_millis()-1,"softwareVersion":"1.1.16"}),
    ] {
        let message: Message = serde_json::from_value(value).unwrap();
        c.observe(Channel::Event, &message, now).unwrap();
    }

    assert_eq!(c.status.ota_trials[0].phase, Phase::Submitted);
    c.disconnected(now).unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);

    let boot: Message = serde_json::from_value(json!({"cmd":"DEVICE_START_EVENT",
        "ts":(now+Duration::seconds(20)).timestamp_millis(),"softwareVersion":"1.1.16"}))
    .unwrap();
    c.status.broker_connected = true;
    c.observe(Channel::Event, &boot, now + Duration::seconds(20))
        .unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::VersionReported);
    assert!(c.status.ota_hold);

    // A host clock correction must not make the earlier request the latest one.
    c.status.version_reported_at = Some(now - Duration::seconds(2));
    c.status.power_reported_at = Some(now - Duration::seconds(2));
    c.status.last_seen = Some(now - Duration::seconds(2));
    let restore = input(Image::Original);
    let (record, command) = submit(&mut c, restore.clone(), now - Duration::seconds(1)).unwrap();
    let command = command.unwrap();

    assert_eq!(command.payload["targetSoftwareVersion"], "1.1.15");
    assert_eq!(
        command.payload["sha256"],
        wellfed_local::ota_image::ORIGINAL_SHA256
    );
    assert!(
        command.payload["url"]
            .as_str()
            .unwrap()
            .ends_with("/original.bin")
    );

    let boot: Message = serde_json::from_value(json!({"cmd":"DEVICE_START_EVENT",
        "ts":(now+Duration::seconds(40)).timestamp_millis(),"softwareVersion":"1.1.15"}))
    .unwrap();
    c.observe(Channel::Event, &boot, now + Duration::seconds(40))
        .unwrap();

    assert!(!c.status.ota_hold);
    assert_eq!(
        c.status.ota_trials[0].input.request_id,
        record.input.request_id
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::VersionReported);
}

#[test]
fn precheck_rejection_storage_failure_and_wrong_request_errors_do_not_resend() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let request = input(Image::Test);
    let now = Utc::now();
    submit(&mut c, request.clone(), now).unwrap();

    for id in [Uuid::new_v4(), request.request_id] {
        let message: Message = serde_json::from_value(
            json!({"cmd":"OTA_UPGRADE","msgId":id,"phase":"precheck","errCode":1101}),
        )
        .unwrap();
        c.observe(Channel::Service, &message, now).unwrap();
        assert_eq!(
            c.status.ota_trials[0].phase,
            if id == request.request_id {
                Phase::ReportedError
            } else {
                Phase::Submitted
            }
        );
        assert_eq!(c.status.ota_hold, id != request.request_id);
    }

    assert!(submit(&mut c, request, now).unwrap().1.is_none());
    assert!(matches!(
        submit(&mut c, input(Image::Test), now),
        Err(Rejection::OtaActive)
    ));

    // Exercise storage failure independently of the unresolved rejection.
    drop(c);
    let path = dir.path().join("storage-ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();
    let now = Utc::now();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();

    assert!(matches!(
        submit(&mut c, input(Image::Test), now),
        Err(Rejection::Storage)
    ));
    assert!(matches!(
        submit(&mut c, input(Image::Test), now),
        Err(Rejection::Storage)
    ));
}

#[test]
fn an_error_after_progress_cannot_clear_the_persistent_feeding_hold() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = Controller::load(common::config(), path.clone()).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let request = input(Image::Test);
    let now = Utc::now();
    submit(&mut c, request.clone(), now).unwrap();

    for value in [
        json!({"cmd":"OTA_PROGRESS","msgId":request.request_id,"progress":10}),
        json!({"cmd":"OTA_UPGRADE","msgId":request.request_id,"phase":"precheck","errCode":1101}),
    ] {
        c.observe(
            Channel::System,
            &serde_json::from_value(value).unwrap(),
            now,
        )
        .unwrap();
    }

    assert!(c.status.ota_hold);
    assert!(c.status.ota_trials[0].download_reported);
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReportedError);

    assert!(submit(&mut c, request, now).unwrap().1.is_none());
    assert!(matches!(
        submit(&mut c, input(Image::Test), now),
        Err(Rejection::OtaActive)
    ));
    assert_eq!(c.status.ota_trials.len(), 1);

    drop(c);
    let mut c = Controller::load(common::config(), path).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();
    assert!(matches!(
        submit(&mut c, input(Image::Test), Utc::now()),
        Err(Rejection::OtaActive)
    ));
    assert!(c.status.ota_hold);
}

#[test]
fn dispatch_requires_a_matching_unexpired_image_server_lease_and_fresh_power() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let now = Utc::now();
    let request = input(Image::Test);
    let mut expiring = lease(request.server_token, now);
    expiring.expires_at = now + Duration::minutes(4);

    for offered in [None, Some(lease(Uuid::new_v4(), now)), Some(expiring)] {
        assert!(matches!(
            c.ota_trial(request.clone(), offered.as_ref(), now),
            Err(Rejection::ImageServer)
        ));
    }

    assert!(c.status.ota_trials.is_empty());
    assert!(!c.status.ota_hold);

    c.status.power_reported_at = Some(now - Duration::seconds(301));
    assert!(matches!(
        submit(&mut c, request.clone(), now),
        Err(Rejection::InvalidOta)
    ));

    c.status.power_reported_at = Some(now);
    assert!(submit(&mut c, request, now).unwrap().1.is_some());
}

#[test]
fn already_active_precheck_rejection_keeps_the_feeding_hold() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let request = input(Image::Test);
    let now = Utc::now();
    submit(&mut c, request.clone(), now).unwrap();

    observe(
        &mut c,
        "service",
        json!({"cmd":"OTA_UPGRADE","msgId":request.request_id,"phase":"precheck","errCode":0x464}),
        now,
    );

    assert_eq!(c.status.ota_trials[0].phase, Phase::ReportedError);
    assert!(c.status.ota_hold);
}

#[test]
fn uncertain_test_resolves_only_after_quiet_period_with_fresh_version_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let t0 = Utc::now();
    c.connected(t0);
    let request = input(Image::Test);
    submit(&mut c, request.clone(), t0).unwrap();

    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), t0),
        Err(Rejection::Conflict)
    ));

    keep_connected(&mut c, t0 + Duration::seconds(600));
    c.expire(t0 + Duration::seconds(600)).unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);

    let config = json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.15","powerType":1});
    // Too soon after submission, even with fresh evidence.
    keep_connected(&mut c, t0 + Duration::minutes(15));
    observe(&mut c, "system", config.clone(), t0 + Duration::minutes(15));
    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), t0 + Duration::minutes(15)),
        Err(Rejection::ResolutionEvidence)
    ));

    // An OTA report restarts the quiet period.
    keep_connected(&mut c, t0 + Duration::minutes(20));
    observe(
        &mut c,
        "system",
        json!({"cmd":"OTA_PROGRESS","msgId":request.request_id,"progress":40}),
        t0 + Duration::minutes(20),
    );
    keep_connected(&mut c, t0 + Duration::minutes(25));
    observe(&mut c, "system", config.clone(), t0 + Duration::minutes(25));
    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), t0 + Duration::minutes(25)),
        Err(Rejection::ResolutionEvidence)
    ));

    // Stale version evidence is refused.
    keep_connected(&mut c, t0 + Duration::minutes(31));
    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), t0 + Duration::minutes(31)),
        Err(Rejection::ResolutionEvidence)
    ));

    let at = t0 + Duration::minutes(31);
    observe(&mut c, "system", config, at);

    let blank = ResolveInput {
        request_id: request.request_id,
        note: " ".to_owned(),
    };
    assert!(matches!(
        c.resolve_trial(blank, at),
        Err(Rejection::Conflict)
    ));

    let record = c.resolve_trial(resolve(request.request_id), at).unwrap();
    assert_eq!(record.phase, Phase::NotInstalled);
    assert!(record.resolution_note.is_some());
    assert!(!c.status.ota_hold);

    // Resolution is not repeatable and permits a fresh explicit request.
    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), at),
        Err(Rejection::Conflict)
    ));
    assert!(submit(&mut c, input(Image::Test), at).unwrap().1.is_some());
}

#[test]
fn resolved_test_that_later_boots_restores_the_hold() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let t0 = Utc::now();
    c.connected(t0);
    let request = input(Image::Test);
    submit(&mut c, request.clone(), t0).unwrap();

    keep_connected(&mut c, t0 + Duration::seconds(600));
    c.expire(t0 + Duration::seconds(600)).unwrap();

    let at = t0 + Duration::minutes(25);
    keep_connected(&mut c, at);
    observe(
        &mut c,
        "system",
        json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.15"}),
        at,
    );

    c.resolve_trial(resolve(request.request_id), at).unwrap();
    assert!(!c.status.ota_hold);

    observe(
        &mut c,
        "event",
        json!({"cmd":"DEVICE_START_EVENT","ts":at.timestamp_millis(),"softwareVersion":"1.1.16"}),
        at,
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::VersionReported);
    assert!(c.status.ota_hold);
}

#[test]
fn uncertain_restore_resolves_by_reported_version_and_hold_follows_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let t0 = Utc::now();
    c.connected(t0);
    let test = input(Image::Test);
    submit(&mut c, test, t0).unwrap();

    let t1 = t0 + Duration::minutes(3);
    keep_connected(&mut c, t1);
    observe(
        &mut c,
        "event",
        json!({"cmd":"DEVICE_START_EVENT","ts":t1.timestamp_millis(),
            "softwareVersion":"1.1.16","powerType":1}),
        t1,
    );

    let restore = input(Image::Original);
    submit(&mut c, restore.clone(), t1).unwrap();
    keep_connected(&mut c, t1 + Duration::seconds(600));
    c.expire(t1 + Duration::seconds(600)).unwrap();

    let busy = t1 + Duration::minutes(11);
    keep_connected(&mut c, busy);
    observe(
        &mut c,
        "system",
        json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.16","powerType":1}),
        busy,
    );

    assert!(matches!(
        submit(&mut c, input(Image::Original), busy),
        Err(Rejection::OtaActive)
    ));

    // Still on the test image: closed as not installed, hold kept, retry allowed.
    let at = t1 + Duration::minutes(25);
    keep_connected(&mut c, at);
    observe(
        &mut c,
        "system",
        json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.16","powerType":1}),
        at,
    );

    assert_eq!(
        c.resolve_trial(resolve(restore.request_id), at)
            .unwrap()
            .phase,
        Phase::NotInstalled
    );
    assert!(c.status.ota_hold);

    let retry = input(Image::Original);
    submit(&mut c, retry.clone(), at).unwrap();
    keep_connected(&mut c, at + Duration::seconds(600));
    c.expire(at + Duration::seconds(600)).unwrap();

    let later = at + Duration::minutes(25);
    keep_connected(&mut c, later);
    observe(
        &mut c,
        "system",
        json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.15","powerType":1}),
        later,
    );

    // The earlier resolved record is neither latest nor unresolved.
    assert!(matches!(
        c.resolve_trial(resolve(restore.request_id), later),
        Err(Rejection::Conflict)
    ));
    assert_eq!(
        c.resolve_trial(resolve(retry.request_id), later)
            .unwrap()
            .phase,
        Phase::VersionReported
    );
    assert!(!c.status.ota_hold);
}

#[test]
fn feeder_telemetry_gaps_restart_resolution_quiet_time_with_or_without_expiry_ticks() {
    for run_expiry_tick in [false, true] {
        for gap_seconds in [60, 900] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
            ready(&mut c, "1.1.15");
            c.enable_ota_trial();

            let t0 = Utc::now();
            c.connected(t0);
            let request = input(Image::Test);
            submit(&mut c, request.clone(), t0).unwrap();

            keep_connected(&mut c, t0 + Duration::minutes(10));
            c.expire(t0 + Duration::minutes(10)).unwrap();

            let returning = t0 + Duration::minutes(30);
            keep_connected(&mut c, returning - Duration::seconds(gap_seconds));

            // Only the feeder goes silent. The server stays connected to its
            // own broker; the return must reset quiet time even without a tick.
            if run_expiry_tick {
                c.expire(returning).unwrap();
                assert!(!c.status.device_online);
            }

            let config = json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.15"});
            observe(&mut c, "system", config.clone(), returning);
            assert!(c.status.device_online);
            assert!(matches!(
                c.resolve_trial(resolve(request.request_id), returning),
                Err(Rejection::ResolutionEvidence)
            ));
            assert!(c.status.ota_hold);

            let almost = returning + Duration::seconds(599);
            keep_connected(&mut c, almost);
            observe(&mut c, "system", config.clone(), almost);
            assert!(matches!(
                c.resolve_trial(resolve(request.request_id), almost),
                Err(Rejection::ResolutionEvidence)
            ));

            // Ten full observed minutes and fresh version evidence permit
            // resolution without publishing another command.
            let complete = returning + Duration::minutes(10);
            observe(&mut c, "system", config, complete);
            assert_eq!(
                c.resolve_trial(resolve(request.request_id), complete)
                    .unwrap()
                    .phase,
                Phase::NotInstalled
            );
            assert_eq!(c.status.ota_trials.len(), 1);
            assert!(!c.status.ota_hold);
        }
    }
}

#[test]
fn backwards_host_clock_restarts_the_resolution_observation_interval() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Controller::load(common::config(), dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();

    let t0 = Utc::now();
    c.connected(t0);
    let request = input(Image::Test);
    submit(&mut c, request.clone(), t0).unwrap();

    keep_connected(&mut c, t0 + Duration::minutes(25));
    c.expire(t0 + Duration::minutes(25)).unwrap();

    let adjusted = t0 + Duration::minutes(20);
    let config = json!({"cmd":"GET_CONFIG","softwareVersion":"1.1.15"});
    observe(&mut c, "system", config.clone(), adjusted);
    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), adjusted),
        Err(Rejection::ResolutionEvidence)
    ));

    let complete = adjusted + Duration::minutes(10);
    keep_connected(&mut c, complete);
    observe(&mut c, "system", config, complete);
    assert_eq!(
        c.resolve_trial(resolve(request.request_id), complete)
            .unwrap()
            .phase,
        Phase::NotInstalled
    );
}
