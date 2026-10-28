//! Install-trial rules in the controller, without a broker: dispatch guards,
//! journal-before-publish with no replay, closing on the replacement's
//! device-API answer, and
//! evidence-based resolution of uncertain or failed trials.

mod common;

use chrono::{DateTime, Duration, Utc};
use common::Controller;
use serde_json::json;
use uuid::Uuid;
use wellfed_stock::protocol::Channel;
use wellfed_stock::{
    controller::Rejection,
    ota::{
        self,
        image::Lease,
        trial::{AnsweredInput, Phase, ResolveInput},
    },
    protocol::{Message, Reply},
};

/// The served build's identity, as its frozen manifest and its device API
/// name it.
fn served() -> String {
    "cd".repeat(32)
}

/// A live image-server lease for `token`, serving [`served`] with digest
/// `ab…`.
fn lease(token: Uuid, now: DateTime<Utc>) -> Lease {
    Lease {
        token,
        peer: "10.0.0.2".parse().unwrap(),
        expires_at: now + Duration::minutes(15),
        sha256: "ab".repeat(32),
        build: served(),
    }
}

/// Submit with a live image-server lease matching the request's token.
fn submit(
    c: &mut Controller,
    input: ota::trial::Input,
    now: DateTime<Utc>,
) -> Result<(ota::trial::Record, Option<Reply>), Rejection> {
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

fn input() -> ota::trial::Input {
    ota::trial::Input {
        request_id: Uuid::new_v4(),
        server_token: Uuid::new_v4(),
    }
}

/// A controller with trials enabled and a freshly reported, powered stock.
fn enabled(dir: &tempfile::TempDir) -> Controller {
    let mut c = common::load(dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();
    c
}

#[test]
fn default_disabled_and_guarded_by_version_power_and_plans() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = common::load(dir.path().join("ledger.json")).unwrap();
    ready(&mut c, "1.1.15");

    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::OtaDisabled)
    ));

    c.enable_ota_trial();
    c.status.version_reported_at = None;
    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    c.status.version_reported_at = Some(Utc::now() - Duration::seconds(301));
    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    for version in ["1.1.14", "1.1.16", "2.0.0", "unverified"] {
        ready(&mut c, version);
        assert!(matches!(
            submit(&mut c, input(), Utc::now()),
            Err(Rejection::InvalidOta)
        ));
    }

    ready(&mut c, "1.1.15");
    c.status.attributes.insert("powerType".to_owned(), json!(2));
    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    c.status.attributes.insert("powerType".to_owned(), json!(1));
    c.config.plans = Some(vec![json!({"id":"plan"})]);
    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::InvalidOta)
    ));

    assert!(c.status.ota_trials.is_empty());

    // No snapshot (an installer's config): stock's own plans are left for
    // the installer to import, so the trial may go ahead.
    c.config.plans = None;
    assert!(submit(&mut c, input(), Utc::now()).is_ok());
}

#[test]
fn journal_before_publish_duplicate_conflict_restart_and_timeout_never_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = enabled(&dir);

    let request = input();
    let now = Utc::now();
    let (record, command) = submit(&mut c, request.clone(), now).unwrap();
    assert!(command.is_some());

    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        on_disk["ota_trials"][request.request_id.to_string()]["phase"],
        "submitted"
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
    let mut c = common::load(path).unwrap();
    assert!(!c.status.ota_trial_enabled);
    assert!(submit(&mut c, request, now).unwrap().1.is_none());

    // With fresh evidence, the uncertain trial alone refuses a new one.
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();
    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::OtaActive)
    ));
}

#[test]
fn a_replacement_install_announces_the_served_digest_and_needs_stock() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);
    let digest = "cd".repeat(32);

    let now = Utc::now();
    let request = input();
    let images = Lease {
        sha256: digest.clone(),
        ..lease(request.server_token, now)
    };

    let (record, command) = c.ota_trial(request.clone(), Some(&images), now).unwrap();
    let command = command.unwrap();
    assert_eq!(record.image_sha256, digest);
    assert_eq!(command.payload["sha256"], digest.as_str());
    assert_eq!(command.payload["targetSoftwareVersion"], "2.0.0");
    assert_eq!(command.payload["targetPid"], "PLAF103");
    assert_eq!(command.payload["upgradeType"], 1);

    // The stock URL parser copies into 128 bytes.
    let url = command.payload["url"].as_str().unwrap();
    assert!(url.starts_with("http://") && url.ends_with("/replacement.bin"));
    assert!(url.len() < 128, "{url}");

    // The same ID returns the record without a second command.
    let (_, again) = c.ota_trial(request, Some(&images), now).unwrap();
    assert!(again.is_none());
}

#[test]
fn progress_and_stale_startup_cannot_close_a_trial() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);

    let request = input();
    let now = Utc::now();
    submit(&mut c, request.clone(), now).unwrap();

    for value in [
        json!({"cmd":"OTA_PROGRESS","msgId":request.request_id,"progress":100}),
        json!({"cmd":"DEVICE_START_EVENT","ts":now.timestamp_millis()-1,"softwareVersion":"2.0.0"}),
    ] {
        let message: Message = serde_json::from_value(value).unwrap();
        c.observe(Channel::Event, &message, now).unwrap();
    }

    assert_eq!(c.status.ota_trials[0].phase, Phase::Submitted);
    assert!(c.status.ota_trials[0].download_reported);
    c.disconnected(now).unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);
}

#[test]
fn precheck_rejection_storage_failure_and_wrong_request_errors_do_not_resend() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);

    let request = input();
    let now = Utc::now();
    submit(&mut c, request.clone(), now).unwrap();

    for id in [Uuid::new_v4(), request.request_id] {
        observe(
            &mut c,
            "service",
            json!({"cmd":"OTA_UPGRADE","msgId":id,"phase":"precheck","errCode":1101}),
            now,
        );
        assert_eq!(
            c.status.ota_trials[0].phase,
            if id == request.request_id {
                Phase::ReportedError
            } else {
                Phase::Submitted
            }
        );
    }

    assert!(submit(&mut c, request, now).unwrap().1.is_none());
    assert!(matches!(
        submit(&mut c, input(), now),
        Err(Rejection::OtaActive)
    ));

    // Exercise storage failure independently of the unresolved rejection.
    drop(c);
    let path = dir.path().join("storage-ledger.json");
    let mut c = common::load(path.clone()).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();
    let now = Utc::now();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();

    assert!(matches!(
        submit(&mut c, input(), now),
        Err(Rejection::Storage)
    ));
    assert!(matches!(
        submit(&mut c, input(), now),
        Err(Rejection::Storage)
    ));
}

#[test]
fn an_error_after_progress_stays_failed_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = enabled(&dir);

    let request = input();
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

    assert!(c.status.ota_trials[0].download_reported);
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReportedError);
    assert!(submit(&mut c, request, now).unwrap().1.is_none());
    assert_eq!(c.status.ota_trials.len(), 1);

    drop(c);
    let mut c = common::load(path).unwrap();
    ready(&mut c, "1.1.15");
    c.enable_ota_trial();
    assert!(matches!(
        submit(&mut c, input(), Utc::now()),
        Err(Rejection::OtaActive)
    ));
}

#[test]
fn dispatch_requires_a_matching_unexpired_image_server_lease_and_fresh_power() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);

    let now = Utc::now();
    let request = input();
    let mut expiring = lease(request.server_token, now);
    expiring.expires_at = now + Duration::minutes(4);

    for offered in [None, Some(lease(Uuid::new_v4(), now)), Some(expiring)] {
        assert!(matches!(
            c.ota_trial(request.clone(), offered.as_ref(), now),
            Err(Rejection::ImageServer)
        ));
    }
    assert!(c.status.ota_trials.is_empty());

    c.status.power_reported_at = Some(now - Duration::seconds(301));
    assert!(matches!(
        submit(&mut c, request.clone(), now),
        Err(Rejection::InvalidOta)
    ));

    c.status.power_reported_at = Some(now);
    assert!(submit(&mut c, request, now).unwrap().1.is_some());
}

fn answered(id: Uuid) -> AnsweredInput {
    AnsweredInput {
        request_id: id,
        build: served(),
        bank: 0,
        note: "build cdcdcdcdcdcd answered the device API from bank 0".to_owned(),
    }
}

#[test]
fn the_replacements_answer_closes_only_the_latest_open_install() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = enabled(&dir);
    let now = Utc::now();
    let request = input();

    // No trial with that ID.
    assert_eq!(
        c.replacement_answered(answered(request.request_id), now),
        Err(Rejection::Conflict)
    );

    submit(&mut c, request.clone(), now).unwrap();

    // An answer stamped at the submission itself proves nothing, and a
    // blank note is refused.
    assert_eq!(
        c.replacement_answered(answered(request.request_id), now),
        Err(Rejection::NotOpen)
    );
    let blank = AnsweredInput {
        note: " ".into(),
        ..answered(request.request_id)
    };
    assert_eq!(
        c.replacement_answered(blank, now + Duration::seconds(200)),
        Err(Rejection::Conflict)
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::Submitted);

    let later = now + Duration::seconds(200);
    let record = c
        .replacement_answered(answered(request.request_id), later)
        .unwrap();
    assert_eq!(record.phase, Phase::ReplacementHealthy);
    assert_eq!(record.build, Some(served()));
    assert_eq!(
        record.resolution_note,
        Some(answered(request.request_id).note)
    );

    // A repeat returns the closed record; a different note is refused.
    assert_eq!(
        c.replacement_answered(answered(request.request_id), later),
        Ok(record)
    );
    let other = AnsweredInput {
        note: "something else".into(),
        ..answered(request.request_id)
    };
    assert_eq!(
        c.replacement_answered(other, later),
        Err(Rejection::NotOpen)
    );

    // Persisted, and a restart does not reopen it.
    drop(c);
    let c = common::load(path).unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReplacementHealthy);
}

#[test]
fn an_answer_is_refused_while_stock_still_reports() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);
    let now = Utc::now();
    let request = input();
    submit(&mut c, request.clone(), now).unwrap();

    // Stock's telemetry is current: it is the one running.
    let later = now + Duration::seconds(200);
    observe(
        &mut c,
        "heart",
        json!({"cmd":"HEARTBEAT"}),
        later - Duration::seconds(10),
    );
    assert_eq!(
        c.replacement_answered(answered(request.request_id), later),
        Err(Rejection::StockRunning)
    );

    // It dropped mid-download, came back on 1.1.15 and reported its
    // version, then went quiet: the trial stays open for a resolution, even
    // with its telemetry stale and the session gone.
    c.disconnected(later).unwrap();
    c.connected(later);
    observe(
        &mut c,
        "event",
        json!({"cmd":"ATTR_PUSH_EVENT","softwareVersion":"1.1.15","powerType":1}),
        later + Duration::seconds(5),
    );
    c.disconnected(later + Duration::seconds(10)).unwrap();
    assert!(c.status.ota_trials[0].stock_reported);
    assert_eq!(
        c.replacement_answered(answered(request.request_id), later + Duration::seconds(300)),
        Err(Rejection::StockRunning)
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);

    // Nor does a restart forget it.
    drop(c);
    let mut c = common::load(dir.path().join("ledger.json")).unwrap();
    assert_eq!(
        c.replacement_answered(answered(request.request_id), later + Duration::seconds(400)),
        Err(Rejection::StockRunning)
    );
}

#[test]
fn an_answer_must_name_the_served_build_on_bank_0() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);
    let now = Utc::now();
    let request = input();
    submit(&mut c, request.clone(), now).unwrap();
    assert_eq!(c.status.ota_trials[0].build, Some(served()));
    let later = now + Duration::seconds(200);

    let other_build = AnsweredInput {
        build: "ef".repeat(32),
        ..answered(request.request_id)
    };
    assert_eq!(
        c.replacement_answered(other_build, later),
        Err(Rejection::WrongBuild)
    );
    let bank1 = AnsweredInput {
        bank: 1,
        ..answered(request.request_id)
    };
    assert_eq!(
        c.replacement_answered(bank1, later),
        Err(Rejection::WrongBuild)
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::Submitted);

    // The right build closes it, once.
    c.replacement_answered(answered(request.request_id), later)
        .unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReplacementHealthy);
    let again = AnsweredInput {
        build: "ef".repeat(32),
        ..answered(request.request_id)
    };
    assert_eq!(
        c.replacement_answered(again, later),
        Err(Rejection::NotOpen)
    );
}

#[test]
fn a_record_from_before_the_build_was_kept_never_closes_on_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut c = enabled(&dir);
    let now = Utc::now();
    let request = input();
    submit(&mut c, request.clone(), now).unwrap();
    drop(c);

    // The ledger as an older server wrote it: no build beside the digest.
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let trial = &mut ledger["ota_trials"][request.request_id.to_string()];
    assert!(trial.as_object_mut().unwrap().remove("build").is_some());
    assert!(
        trial
            .as_object_mut()
            .unwrap()
            .remove("stock_reported")
            .is_some()
    );
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();

    let mut c = common::load(path).unwrap();
    assert_eq!(c.status.ota_trials[0].build, None);
    assert_eq!(
        c.replacement_answered(answered(request.request_id), now + Duration::seconds(200)),
        Err(Rejection::WrongBuild)
    );
    // The restart left it uncertain, for a resolution from version evidence.
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);
}

#[test]
fn an_uncertain_install_still_closes_on_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);
    let now = Utc::now();
    let request = input();
    submit(&mut c, request.clone(), now).unwrap();

    // Stock's own session ends when it reboots into the new bank.
    c.disconnected(now + Duration::seconds(100)).unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::Uncertain);

    c.replacement_answered(answered(request.request_id), now + Duration::seconds(200))
        .unwrap();
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReplacementHealthy);
}

#[test]
fn a_failed_install_does_not_close_on_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);
    let now = Utc::now();
    let request = input();
    submit(&mut c, request.clone(), now).unwrap();
    observe(
        &mut c,
        "service",
        json!({"cmd":"OTA_UPGRADE","msgId":request.request_id,"phase":"precheck","errCode":1101}),
        now + Duration::seconds(5),
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReportedError);

    assert_eq!(
        c.replacement_answered(answered(request.request_id), now + Duration::seconds(200)),
        Err(Rejection::NotOpen)
    );
}

#[test]
fn an_uncertain_install_resolves_only_after_a_quiet_period_with_fresh_version_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);

    let t0 = Utc::now();
    c.connected(t0);
    let request = input();
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

    // Resolution is not repeatable and permits a fresh explicit request.
    assert!(matches!(
        c.resolve_trial(resolve(request.request_id), at),
        Err(Rejection::Conflict)
    ));
    assert!(submit(&mut c, input(), at).unwrap().1.is_some());
}

#[test]
fn feeder_telemetry_gaps_restart_resolution_quiet_time_with_or_without_expiry_ticks() {
    for run_expiry_tick in [false, true] {
        for gap_seconds in [90, 900] {
            let dir = tempfile::tempdir().unwrap();
            let mut c = enabled(&dir);

            let t0 = Utc::now();
            c.connected(t0);
            let request = input();
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
        }
    }
}

#[test]
fn backwards_host_clock_restarts_the_resolution_observation_interval() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);

    let t0 = Utc::now();
    c.connected(t0);
    let request = input();
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

#[test]
fn stock_s_61_second_heartbeat_keeps_the_quiet_interval_running() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = enabled(&dir);
    let now = Utc::now();

    let request = input();
    submit(&mut c, request.clone(), now).unwrap();
    observe(
        &mut c,
        "service",
        json!({"cmd":"OTA_INFORM","msgId":request.request_id.to_string(),"errCode":2003}),
        now + Duration::seconds(30),
    );
    assert_eq!(c.status.ota_trials[0].phase, Phase::ReportedError);

    // Heartbeats 61 s apart, as stock 1.1.15 sends them, for 21 minutes,
    // with a fresh version report at the end.
    let mut at = now + Duration::seconds(61);
    while at < now + Duration::minutes(21) {
        observe(&mut c, "heart", json!({"cmd":"HEARTBEAT"}), at);
        at += Duration::seconds(61);
    }
    observe(
        &mut c,
        "event",
        json!({"cmd":"ATTR_PUSH_EVENT","softwareVersion":"1.1.15","powerType":1}),
        at,
    );

    let record = c.resolve_trial(resolve(request.request_id), at).unwrap();
    assert_eq!(record.phase, Phase::NotInstalled);
}
