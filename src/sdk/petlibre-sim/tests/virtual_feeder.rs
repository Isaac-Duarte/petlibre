//! End-to-end replacement Rust behavior on synthetic hardware and persistent NOR.
//!
//! Each test runs the real `virtual-feeder-worker` through the harness, so
//! besides its own expectations every run also checks the safety invariants
//! (reservation before drive, valid motor pair, inactive outputs when
//! durable). This is simulation evidence, not hardware reliability.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use serde_json::{Value, json};

use petlibre_sim::protocol::{Adc, Command, Journal, Profile};
use petlibre_sim::scenario::Scenario;
use petlibre_sim::{Worker, run_scenario};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn worker() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_virtual-feeder-worker"))
}

fn act(at: u64, op: &str, args: &[u64]) -> Value {
    json!({"at_ms": at, "op": op, "args": args})
}

fn expect(at: u64, values: Value) -> Value {
    json!({"at_ms": at, "op": "expect", "values": values})
}

fn scenario(actions: Vec<Value>, end: u64, profile: &str, feedback: u64) -> Value {
    json!({"version": 1, "name": "integration", "profile": profile,
           "feedback_period_ms": feedback, "end_ms": end, "actions": actions})
}

fn run(scenario: &Value) -> Result<Value, petlibre_sim::HarnessError> {
    let report = run_scenario(scenario, &root(), &worker())?;
    Ok(serde_json::to_value(report).unwrap())
}

fn passing(actions: Vec<Value>, end: u64, profile: &str, feedback: u64) -> Value {
    let report = run(&scenario(actions, end, profile, feedback)).unwrap();

    let failed: Vec<&Value> = report["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["passed"] != true)
        .collect();

    assert!(report["passed"] == true, "{failed:?}");
    report
}

fn events<'r>(report: &'r Value, kind: &str) -> impl Iterator<Item = &'r Value> + use<'r> {
    let kind = kind.to_string();
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(move |e| e["event"] == kind.as_str())
}

/// A drive on the boot after a power cut or reset.
fn drove_after_reboot(report: &Value) -> bool {
    events(report, "outputs").any(|e| e["boot"] == 2 && matches!(e["motor"].as_u64(), Some(1 | 2)))
}

#[test]
fn single_and_double_bowl_complete_through_real_journal() {
    for profile in ["single", "double"] {
        let report = passing(
            vec![
                act(1, "submit", &[1, 3]),
                expect(1, json!({"submission": "enqueued"})),
                expect(
                    90,
                    json!({"state": "terminal", "completed": 3, "outcome": "done", "durable": true,
                                  "motor": 0, "optics": 0, "pending": false, "high_water_id": 1}),
                ),
            ],
            95,
            profile,
            10,
        );

        let drives: Vec<u64> = events(&report, "outputs")
            .filter_map(|e| e["motor"].as_u64())
            .collect();
        assert!(drives.contains(&1), "{profile}");
        if profile == "double" {
            assert!(drives.contains(&2));
        }
    }
}

#[test]
fn mailbox_ids_acknowledgement_and_next_request() {
    passing(
        vec![
            act(0, "submit", &[0, 1]),
            expect(0, json!({"submission": "reject_id"})),
            act(0, "submit", &[1, 0]),
            expect(0, json!({"submission": "reject_request"})),
            act(1, "submit", &[1, 1]),
            act(2, "submit", &[2, 1]),
            expect(2, json!({"submission": "reject_busy"})),
            act(8, "cancel", &[99]),
            expect(8, json!({"response": 0})),
            act(25, "ack", &[99]),
            expect(25, json!({"response": 0})),
            expect(
                26,
                json!({"state": "terminal", "durable": true, "completed": 1}),
            ),
            act(27, "ack", &[1]),
            expect(28, json!({"state": "idle"})),
            act(28, "submit", &[1, 1]),
            expect(28, json!({"submission": "reject_id"})),
            act(29, "submit", &[2, 1]),
            expect(29, json!({"submission": "enqueued"})),
            expect(
                70,
                json!({"state": "terminal", "id": 2, "completed": 1, "durable": true}),
            ),
        ],
        80,
        "single",
        10,
    );
}

#[test]
fn interruption_invalidates_double_bowl_phase() {
    passing(
        vec![
            act(1, "submit", &[1, 3]),
            act(8, "cancel", &[1]),
            expect(
                20,
                json!({"outcome": "cancelled", "durable": true, "phase_known": false, "motor": 0}),
            ),
            act(21, "ack", &[1]),
            act(22, "submit", &[2, 1]),
            expect(22, json!({"submission": "reject_phase"})),
        ],
        25,
        "double",
        10,
    );
}

#[test]
fn faults_stop_and_finish_without_success() {
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "missing",
            vec![act(8, "auto", &[0])],
            json!({"reason": "feedback_timeout"}),
        ),
        (
            "outlet",
            vec![act(8, "inputs", &[4])],
            json!({"reason": "outlet"}),
        ),
        (
            "current",
            vec![act(8, "current", &[4095])],
            json!({"reason": "overcurrent"}),
        ),
        (
            "power",
            vec![act(8, "inputs", &[1]), act(8, "battery", &[0])],
            json!({"reason": "power"}),
        ),
        (
            "adc",
            vec![act(8, "adc_stuck", &[1])],
            json!({"state": "fault", "fault": "hardware"}),
        ),
        (
            "timer",
            vec![act(8, "timer", &[0])],
            json!({"state": "fault", "fault": "hardware"}),
        ),
    ];

    for (name, faults, expected) in cases {
        let mut values = json!({"durable": true, "motor": 0, "optics": 0, "completed": 0});
        values
            .as_object_mut()
            .unwrap()
            .extend(expected.as_object().unwrap().clone());

        let mut actions = vec![act(1, "submit", &[1, 3])];
        actions.extend(faults);
        actions.push(expect(50, values));

        let report = passing(actions, 55, "single", 10);
        assert!(
            !events(&report, "status").any(|e| e["outcome"] == 1),
            "{name}"
        );
    }
}

#[test]
fn stalled_owner_independent_timer_and_watchdog_reset() {
    let report = passing(
        vec![
            act(1, "submit", &[1, 3]),
            act(8, "owner", &[0]),
            expect(25, json!({"motor": 0, "optics": 0})),
            expect(
                150,
                json!({"boots": 2, "state": "fault", "fault": "recovery", "pending": true, "motor": 0}),
            ),
        ],
        155,
        "single",
        10,
    );

    assert_eq!(events(&report, "watchdog_reset_requested").count(), 1);
}

#[test]
fn reboot_after_completion_rejects_replay_and_accepts_new_id() {
    passing(
        vec![
            act(1, "submit", &[10, 1]),
            expect(25, json!({"durable": true, "completed": 1})),
            act(26, "power_cut", &[]),
            expect(26, json!({"online": false})),
            act(27, "boot", &[]),
            expect(
                27,
                json!({"state": "idle", "high_water_id": 10, "pending": false}),
            ),
            act(28, "submit", &[10, 1]),
            expect(28, json!({"submission": "reject_id"})),
            act(29, "submit", &[11, 1]),
            expect(29, json!({"submission": "enqueued"})),
            expect(60, json!({"durable": true, "completed": 1, "id": 11})),
        ],
        80,
        "single",
        10,
    );
}

#[test]
fn power_cut_before_reservation_does_not_consume_id() {
    passing(
        vec![
            act(1, "submit", &[1, 1]),
            act(1, "power_cut", &[]),
            act(2, "boot", &[]),
            expect(2, json!({"state": "idle", "high_water_id": 0})),
            act(3, "submit", &[1, 1]),
            expect(3, json!({"submission": "enqueued"})),
            expect(30, json!({"completed": 1, "durable": true})),
        ],
        35,
        "single",
        10,
    );
}

#[test]
fn power_cut_during_motion_blocks_recovery_and_new_feed() {
    let report = passing(
        vec![
            act(1, "submit", &[1, 3]),
            expect(8, json!({"state": "feeding", "motor": 1})),
            act(8, "power_cut", &[]),
            act(9, "boot", &[]),
            expect(
                9,
                json!({"state": "fault", "fault": "recovery", "pending": true, "high_water_id": 1}),
            ),
            act(10, "submit", &[2, 1]),
            expect(10, json!({"submission": "reject_stopped"})),
            expect(30, json!({"motor": 0, "optics": 0, "durable": false})),
        ],
        35,
        "single",
        10,
    );

    assert!(!drove_after_reboot(&report));
}

#[test]
fn byte_cuts_at_erase_body_and_commit_boundaries() {
    for phase in [1, 2] {
        for byte in [0, 1, 256, 257, 270, 380, 381, 383, 384] {
            // A committed reservation is pending; a committed completion is idle.
            let expected = if phase == 1 && byte <= 256 {
                json!({"state": "idle", "high_water_id": 0})
            } else if phase == 2 && byte == 384 {
                json!({"state": "idle", "high_water_id": 1, "pending": false})
            } else {
                json!({"state": "fault", "fault": "recovery"})
            };

            let mut values = json!({"motor": 0, "optics": 0});
            values
                .as_object_mut()
                .unwrap()
                .extend(expected.as_object().unwrap().clone());

            let report = passing(
                vec![
                    act(0, "cut", &[phase, byte]),
                    act(1, "submit", &[1, 1]),
                    expect(25, json!({"online": false})),
                    act(26, "boot", &[]),
                    expect(26, values),
                ],
                30,
                "single",
                10,
            );

            assert_eq!(events(&report, "power_cut").count(), 1, "{phase} {byte}");
            assert!(!drove_after_reboot(&report), "{phase} {byte}");
        }
    }
}

#[test]
fn explicit_feedback_replaces_automatic_plant() {
    let report = passing(
        vec![
            act(1, "submit", &[1, 1]),
            act(15, "edge", &[1]),
            expect(
                25,
                json!({"state": "terminal", "completed": 1, "durable": true}),
            ),
        ],
        30,
        "single",
        0,
    );
    assert_eq!(events(&report, "feedback").count(), 0);
}

#[test]
fn identical_scenarios_have_identical_reports() {
    let data = scenario(
        vec![
            act(1, "submit", &[1, 1]),
            expect(30, json!({"completed": 1})),
        ],
        35,
        "single",
        10,
    );
    assert_eq!(run(&data).unwrap(), run(&data).unwrap());
}

#[test]
fn invalid_scenarios_rejected_before_worker_launch() {
    let base = scenario(vec![expect(1, json!({"online": true}))], 80, "single", 10);
    let mut bad = Vec::new();

    // Each top-level field invalid.
    for (key, value) in [
        ("version", json!(2)),
        ("end_ms", json!(10001)),
        ("profile", json!("stock")),
        ("feedback_period_ms", json!(true)),
        ("actions", json!([])),
    ] {
        let mut item = base.clone();
        item[key] = value;
        bad.push(item);
    }

    // Each malformed action.
    for event in [
        act(1, "shell", &[0]),
        json!({"at_ms": -1, "op": "status", "args": []}),
        act(1, "current", &[4096]),
        json!({"at_ms": 1, "op": "submit", "args": [-1, 1]}),
        expect(1, json!({"durable": 1})),
        expect(1, json!({"unknown": 1})),
        expect(1, json!({"state": "made_up"})),
        expect(1, json!({})),
        act(1, "tick", &[]),
        act(1, "status", &[1]),
        json!({"at_ms": 1, "op": "expect", "values": {"online": true}, "args": []}),
        json!({"at_ms": 1, "op": "status", "args": [], "values": {"online": true}}),
        json!({"at_ms": 1, "op": "stop", "args": [], "extra": 1}),
    ] {
        let mut item = base.clone();
        item["actions"] = json!([event]);
        bad.push(item);
    }

    // Timestamps must not decrease.
    bad.push(scenario(
        vec![act(2, "status", &[]), expect(1, json!({"online": true}))],
        80,
        "single",
        10,
    ));

    for item in bad {
        assert!(Scenario::from_value(&item).is_err(), "{item}");
    }
}

#[test]
fn invalid_worker_commands_and_unexpected_exit() {
    let scratch = tempfile::tempdir().unwrap();
    let journal = scratch.path().join("journal.bin");
    let mut w = Worker::start(&worker(), &journal, Profile::Single, Journal::Provision).unwrap();
    w.read().unwrap();

    // The worker refuses an out-of-range argument by exiting with status 2.
    let error = w
        .command(&Command::Current { args: [Adc(5000)] })
        .unwrap_err();
    assert!(error.to_string().contains("Worker exited 2"), "{error}");
    w.close();

    // Commanding a powered-off worker is a harness error, not a failed assertion.
    let unpowered = scenario(
        vec![
            act(1, "power_cut", &[]),
            act(2, "submit", &[1, 1]),
            expect(3, json!({"online": false})),
        ],
        80,
        "single",
        10,
    );

    assert!(run(&unpowered).is_err());
}

#[test]
fn cli_exit_codes_report_creation_and_no_overwrite() {
    let scratch = tempfile::tempdir().unwrap();
    let data = scratch.path().join("scenario.json");
    let report = scratch.path().join("report.json");

    let cli = |report: &Path| {
        Process::new(env!("CARGO_BIN_EXE_virtual-feeder"))
            .arg("--scenario")
            .arg(&data)
            .arg("--report")
            .arg(report)
            .arg("--root")
            .arg(root())
            .arg("--worker")
            .arg(worker())
            .output()
            .unwrap()
            .status
            .code()
    };

    std::fs::write(
        &data,
        scenario(vec![expect(1, json!({"online": true}))], 2, "single", 10).to_string(),
    )
    .unwrap();
    assert_eq!(cli(&report), Some(0));
    assert_eq!(
        cli(&report),
        Some(2),
        "an existing report is never replaced"
    );

    std::fs::write(
        &data,
        scenario(vec![expect(1, json!({"completed": 1}))], 2, "single", 10).to_string(),
    )
    .unwrap();
    assert_eq!(cli(&scratch.path().join("failure.json")), Some(1));

    std::fs::write(&data, r#"{"version":2}"#).unwrap();
    assert_eq!(cli(&scratch.path().join("refused.json")), Some(2));
}

#[test]
fn checked_in_examples() {
    let directory = root().join("src/firmware/vendor/sim/scenarios");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 8);

    for path in paths {
        let scenario: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let report = run(&scenario).unwrap();
        assert!(report["passed"] == true, "{}", path.display());
    }
}
