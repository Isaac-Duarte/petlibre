//! End-to-end replacement Rust behavior on synthetic hardware and persistent
//! NOR, under the PLAF103 firmware's feeding limits and the measured plant.
//!
//! Each test runs the real `virtual-feeder-worker` through the harness, so
//! besides its own expectations every run also checks the safety invariants
//! (reservation before drive, valid motor pair, inactive outputs when
//! durable, no flash lease while driving). This is simulation evidence, not
//! hardware reliability.
//!
//! Times are the firmware's: a feed submitted at 100 ms starts driving
//! about 120 ms later (the emitters' settling), and each portion is a mark
//! about 9 s of drive apart, so a one-portion feed is over by 10 s.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use serde_json::{Value, json};

use wellfed_sim::protocol::{Adc, Command, Journal, Profile};
use wellfed_sim::scenario::Scenario;
use wellfed_sim::{Worker, run_scenario};

/// When the tests submit their first feed, and when one portion is done.
const SUBMIT: u64 = 100;
const ONE_PORTION: u64 = 10_000;
const THREE_PORTIONS: u64 = 30_000;

/// A double bowl's mechanism was never measured, and the owner credits it a
/// portion on every second mark: its scenarios put the marks 5.5 s apart, so
/// two fit the PLAF103's 12 s a portion.
fn double_plant() -> Value {
    json!({"period_ms": 5500, "low_ms": 600})
}

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

fn scenario(actions: Vec<Value>, end: u64, profile: &str) -> Value {
    let plant = if profile == "double" {
        double_plant()
    } else {
        json!({})
    };
    with_plant(actions, end, profile, plant)
}

fn with_plant(actions: Vec<Value>, end: u64, profile: &str, plant: Value) -> Value {
    json!({"version": 2, "name": "integration", "profile": profile, "plant": plant,
           "end_ms": end, "actions": actions})
}

fn run(scenario: &Value) -> Result<Value, wellfed_sim::HarnessError> {
    let report = run_scenario(scenario, &root(), &worker())?;
    Ok(serde_json::to_value(report).unwrap())
}

fn checked(scenario: &Value) -> Value {
    let report = run(scenario).unwrap();

    let failed: Vec<&Value> = report["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["passed"] != true)
        .collect();

    assert!(report["passed"] == true, "{failed:?}");
    report
}

fn passing(actions: Vec<Value>, end: u64, profile: &str) -> Value {
    checked(&scenario(actions, end, profile))
}

fn events<'r>(report: &'r Value, kind: &str) -> impl Iterator<Item = &'r Value> + use<'r> {
    let kind = kind.to_string();
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(move |e| e["event"] == kind.as_str())
}

/// Every status the worker reported: those that end a command and those
/// that changed while time stepped.
fn statuses(report: &Value) -> impl Iterator<Item = &Value> {
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["event"] == "status" || e["event"] == "changed")
}

fn last_status(report: &Value) -> &Value {
    statuses(report).last().expect("the worker reported")
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
                act(SUBMIT, "submit", &[1, 3]),
                expect(SUBMIT, json!({"submission": "enqueued"})),
                expect(
                    40_000,
                    json!({"state": "terminal", "completed": 3, "outcome": "done", "durable": true,
                           "motor": 0, "optics": 0, "pending": false, "high_water_id": 1}),
                ),
            ],
            40_100,
            profile,
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

/// The plant's edges come where the bench saw them: a portion on each
/// mark's rise, about 9.07 s of drive apart, each mark about 1.19 s LOW.
#[test]
fn a_feed_meets_the_marks_where_the_bench_saw_them() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            expect(
                THREE_PORTIONS,
                json!({"state": "terminal", "completed": 3, "outcome": "done"}),
            ),
        ],
        THREE_PORTIONS,
        "single",
    );

    let start = events(&report, "outputs")
        .find(|e| e["motor"] == 1)
        .and_then(|e| e["at_ms"].as_u64())
        .unwrap();
    let edges: Vec<(u64, bool)> = events(&report, "feedback")
        .map(|e| (e["at_ms"].as_u64().unwrap() - start, e["high"] == true))
        .collect();
    let bench = [7840, 9037, 16970, 18156, 26047, 27221];
    assert_eq!(edges.len(), bench.len(), "{edges:?}");
    for ((at, high), (index, want)) in edges.iter().zip(bench.iter().enumerate()) {
        assert_eq!(*high, index % 2 == 1);
        assert!(at.abs_diff(*want) <= 100, "{at} for {want}");
    }

    // The motor stops at the third rise, within one sample period.
    let stop = events(&report, "outputs")
        .filter(|e| e["motor"] == 0)
        .filter_map(|e| e["at_ms"].as_u64())
        .find(|&at| at > start)
        .unwrap();
    assert!(stop - start - edges[5].0 <= 40, "stopped {stop}");
}

#[test]
fn mailbox_ids_acknowledgement_and_next_request() {
    passing(
        vec![
            act(50, "submit", &[0, 1]),
            expect(50, json!({"submission": "reject_id"})),
            act(50, "submit", &[1, 0]),
            expect(50, json!({"submission": "reject_request"})),
            act(SUBMIT, "submit", &[1, 1]),
            act(SUBMIT + 1, "submit", &[2, 1]),
            expect(SUBMIT + 1, json!({"submission": "reject_busy"})),
            act(3000, "cancel", &[99]),
            expect(3000, json!({"response": 0})),
            act(5000, "ack", &[99]),
            expect(5000, json!({"response": 0})),
            expect(
                ONE_PORTION,
                json!({"state": "terminal", "durable": true, "completed": 1}),
            ),
            act(ONE_PORTION + 1, "ack", &[1]),
            expect(ONE_PORTION + 50, json!({"state": "idle"})),
            act(ONE_PORTION + 50, "submit", &[1, 1]),
            expect(ONE_PORTION + 50, json!({"submission": "reject_id"})),
            act(ONE_PORTION + 51, "submit", &[2, 1]),
            expect(ONE_PORTION + 51, json!({"submission": "enqueued"})),
            expect(
                2 * ONE_PORTION + 100,
                json!({"state": "terminal", "id": 2, "completed": 1, "durable": true}),
            ),
        ],
        2 * ONE_PORTION + 200,
        "single",
    );
}

#[test]
fn interruption_invalidates_double_bowl_phase() {
    passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            act(3000, "cancel", &[1]),
            expect(
                3100,
                json!({"outcome": "cancelled", "durable": true, "phase_known": false, "motor": 0}),
            ),
            act(3101, "ack", &[1]),
            act(3200, "submit", &[2, 1]),
            expect(3200, json!({"submission": "reject_phase"})),
        ],
        3300,
        "double",
    );
}

/// A cancelled drive leaves the rotor between marks. Stopped about 5.1 s
/// past one, the next mark's rise comes 4 s into the next feed, inside the
/// 5 s inhibit: the homing mark, never credited. The portion is the
/// compartment after it, about 13 s from the drive's start, past the 12 s a
/// portion allows from a mark: the homing window extends the deadline.
#[test]
fn a_feed_after_a_cancel_homes_the_rotor_then_counts_from_the_mark() {
    let report = passing(
        vec![
            expect(SUBMIT, json!({"at_mark": false})),
            act(SUBMIT, "submit", &[1, 1]),
            expect(ONE_PORTION, json!({"outcome": "done", "at_mark": true})),
            act(ONE_PORTION + 1, "ack", &[1]),
            act(ONE_PORTION + 100, "submit", &[2, 1]),
            act(ONE_PORTION + 5300, "cancel", &[2]),
            expect(
                ONE_PORTION + 5400,
                json!({"outcome": "cancelled", "durable": true, "at_mark": false, "motor": 0}),
            ),
            act(ONE_PORTION + 5401, "ack", &[2]),
            act(ONE_PORTION + 5500, "submit", &[3, 1]),
            expect(
                ONE_PORTION + 5500 + 14_000,
                json!({"state": "terminal", "id": 3, "outcome": "done", "completed": 1,
                       "homed": true, "at_mark": true}),
            ),
        ],
        ONE_PORTION + 5500 + 14_000,
        "single",
    );

    let drives: Vec<u64> = events(&report, "outputs")
        .filter(|e| e["motor"] == 1)
        .filter_map(|e| e["at_ms"].as_u64())
        .collect();
    let stop = events(&report, "outputs")
        .filter(|e| e["motor"] == 0)
        .filter_map(|e| e["at_ms"].as_u64())
        .find(|&at| at > drives[2])
        .unwrap();
    let took = stop - drives[2];
    assert!(
        took > 12_000 && took < 14_000,
        "the homed feed drove {took} ms"
    );
}

/// A short cancelled drive leaves the next mark past the inhibit: it is
/// credited as stock credits it, with no homing mark.
#[test]
fn a_mark_past_the_inhibit_is_credited_even_while_homing() {
    passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(2000, "cancel", &[1]),
            expect(2100, json!({"outcome": "cancelled", "at_mark": false})),
            act(2101, "ack", &[1]),
            act(2200, "submit", &[2, 1]),
            expect(
                2200 + ONE_PORTION,
                json!({"state": "terminal", "outcome": "done", "completed": 1,
                       "homed": false, "at_mark": true}),
            ),
        ],
        2200 + ONE_PORTION,
        "single",
    );
}

/// The phase follows the journal's last result across a reboot: a
/// completed feed leaves the rotor at a mark, a feed a power cut
/// interrupted resolves uncertain and leaves it unknown, and the next
/// completed feed sets it again.
#[test]
fn the_phase_after_a_reboot_follows_the_journal() {
    passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            expect(ONE_PORTION, json!({"outcome": "done", "at_mark": true})),
            act(ONE_PORTION + 1, "ack", &[1]),
            act(ONE_PORTION + 100, "power_cut", &[]),
            act(ONE_PORTION + 101, "boot", &[]),
            expect(ONE_PORTION + 200, json!({"state": "idle", "at_mark": true})),
            act(ONE_PORTION + 300, "submit", &[2, 1]),
            act(ONE_PORTION + 3000, "power_cut", &[]),
            act(ONE_PORTION + 3001, "boot", &[]),
            expect(
                ONE_PORTION + 3002,
                json!({"outcome": "uncertain", "at_mark": false}),
            ),
            act(ONE_PORTION + 3003, "ack", &[2]),
            act(ONE_PORTION + 3100, "submit", &[3, 1]),
            expect(
                ONE_PORTION + 3100 + 14_000,
                json!({"state": "terminal", "id": 3, "outcome": "done", "at_mark": true}),
            ),
        ],
        ONE_PORTION + 3100 + 14_000,
        "single",
    );
}

#[test]
fn faults_stop_and_finish_without_success() {
    // Each checked before a hardware fault's watchdog reset, which follows
    // once the faulted owner's beat lapses; lost sensors are held instead.
    let cases: Vec<(&str, Vec<Value>, u64, Value)> = vec![
        (
            "missing",
            vec![act(3000, "jam", &[1])],
            16_000,
            json!({"reason": "feedback_timeout"}),
        ),
        (
            "wedged",
            vec![act(3000, "wedge", &[2])],
            16_000,
            json!({"reason": "jammed"}),
        ),
        (
            "outlet",
            vec![act(3000, "inputs", &[4])],
            5000,
            json!({"reason": "outlet"}),
        ),
        (
            "current",
            vec![act(3000, "current", &[4095])],
            5000,
            json!({"reason": "overcurrent"}),
        ),
        (
            "power",
            vec![act(3000, "inputs", &[1]), act(3000, "battery", &[0])],
            5000,
            json!({"reason": "power"}),
        ),
        (
            "adc",
            vec![act(3000, "adc_stuck", &[1])],
            5000,
            json!({"state": "fault", "fault": "sensor", "reason": "sensor"}),
        ),
        (
            "timer",
            vec![act(3000, "timer", &[0])],
            5000,
            json!({"state": "fault", "fault": "hardware"}),
        ),
    ];

    for (name, faults, at, expected) in cases {
        let mut values = json!({"durable": true, "motor": 0, "optics": 0, "completed": 0});
        values
            .as_object_mut()
            .unwrap()
            .extend(expected.as_object().unwrap().clone());

        let mut actions = vec![act(SUBMIT, "submit", &[1, 3])];
        actions.extend(faults);
        actions.push(expect(at, values));

        let report = passing(actions, at + 100, "single");
        assert!(!statuses(&report).any(|e| e["outcome"] == 1), "{name}");
    }
}

/// The loop's handshake operations during a feed: an enrolled client's
/// session opens while the motor drives, each operation holding the loop
/// for up to about 80 ms (one or two X25519s, measured 25-28 ms each, with
/// preemption). The firmware steps the owner just before such a pass and
/// again just after it, so the owner misses at most that long. Holds of
/// 70 ms every half second, a session opened again and again for the whole
/// feed, leave it to complete.
#[test]
fn handshake_sized_holds_during_a_drive_leave_the_feed_to_complete() {
    let mut actions = vec![act(SUBMIT, "submit", &[1, 3])];
    for at in (1000..26_000).step_by(500) {
        actions.push(act(at, "owner", &[0]));
        actions.push(act(at + 70, "owner", &[1]));
    }
    actions.push(expect(
        THREE_PORTIONS + 2000,
        json!({"state": "terminal", "completed": 3, "outcome": "done", "durable": true,
               "motor": 0, "optics": 0}),
    ));
    passing(actions, THREE_PORTIONS + 2100, "single");
}

/// The worst a stranger could ask during a feed, were each handshake not
/// refused unanswered and the rest held: one operation every pass, each
/// holding the loop about 80 ms, the owner stepped once between them. The
/// holds start at a step (the first one may also take up to a sample
/// period, as the firmware's never does), and the feed still completes.
#[test]
fn back_to_back_handshake_holds_leave_the_feed_to_complete() {
    let mut actions = vec![act(SUBMIT, "submit", &[1, 3])];
    let mut at = 1000;
    while at < 26_000 {
        actions.push(act(at, "owner", &[0]));
        actions.push(act(at + 80, "owner", &[1]));
        at += 81;
    }
    actions.push(expect(
        THREE_PORTIONS + 2000,
        json!({"state": "terminal", "completed": 3, "outcome": "done", "durable": true,
               "motor": 0, "optics": 0}),
    ));
    passing(actions, THREE_PORTIONS + 2100, "single");
}

/// A hold past the owner's 100 ms observation gap ends the feed, stopped
/// and recorded, never driven blind: the budget a handshake operation must
/// stay inside, and what overrunning it costs (a failed feed, not safety).
#[test]
fn a_hold_past_the_observation_gap_fails_the_feed_and_stops() {
    passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            act(3000, "owner", &[0]),
            act(3150, "owner", &[1]),
            expect(
                3300,
                json!({"state": "terminal", "reason": "observation_gap", "durable": true,
                       "motor": 0, "optics": 0, "completed": 0}),
            ),
        ],
        3400,
        "single",
    );
}

/// The supervisor stops the motor within its 200 ms lease; the world's
/// feeder feeds the watchdog until the owner's beat is a gap (1 s while
/// outputs may move) old, and the 4 s expiry then resets the board. The
/// next boot resolves the interrupted feed as uncertain.
#[test]
fn stalled_owner_independent_timer_and_watchdog_reset() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            act(3000, "owner", &[0]),
            expect(3300, json!({"motor": 0})),
            expect(6000, json!({"boots": 1, "motor": 0})),
            expect(
                10_000,
                json!({"boots": 2, "state": "terminal", "fault": "ok", "outcome": "uncertain",
                       "durable": true, "pending": false, "motor": 0}),
            ),
        ],
        10_100,
        "single",
    );

    let reset = events(&report, "watchdog_reset_requested")
        .map(|e| e["at_ms"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(reset.len(), 1);
    // The beat's 1 s gap, then an expiry between the watchdog's bounds.
    assert!((3000 + 1000 + 2426..=3000 + 1000 + 5460).contains(&reset[0]));
}

/// When the motor first read stopped at or after `from`.
fn stopped_at(report: &Value, from: u64) -> u64 {
    events(report, "outputs")
        .filter(|e| e["motor"] == 0)
        .filter_map(|e| e["at_ms"].as_u64())
        .find(|&at| at >= from)
        .expect("the motor stopped")
}

/// The `feed-stall` variant's short stall: the loop holds 600 ms, past the
/// 200 ms motor lease and inside the 1 s beat. The supervisor stops the
/// motor; the owner, stepped again, records the feed failed (output)
/// durably, and its lapsed beat then resets the board, whose next boot is
/// idle with nothing pending.
#[test]
fn a_stall_inside_the_beat_stops_by_the_lease_and_records_the_feed() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(2200, "owner", &[0]),
            act(2800, "owner", &[1]),
            expect(
                2900,
                json!({"boots": 1, "state": "fault", "fault": "hardware", "outcome": "failed",
                       "reason": "output", "durable": true, "pending": false, "motor": 0}),
            ),
            expect(
                10_000,
                json!({"boots": 2, "state": "idle", "pending": false, "high_water_id": 1,
                       "motor": 0}),
            ),
        ],
        10_100,
        "single",
    );

    let stopped = stopped_at(&report, 2200);
    assert!((2200..=2200 + 200 + 20).contains(&stopped), "{stopped}");
}

/// The `stop-fail` variant's short hold: a cancel's stop does not take for
/// 300 ms. The timer interrupt retries it every tick and the first retry
/// after the hold stops the motor; the cancellation is then recorded
/// durably, before the faulted owner's lapsed beat resets the board.
#[test]
fn a_stop_that_does_not_take_is_retried_until_it_does() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(2200, "stop_fails", &[1]),
            act(2200, "cancel", &[1]),
            expect(2490, json!({"motor": 1, "durable": false})),
            act(2500, "stop_fails", &[0]),
            expect(
                2600,
                json!({"boots": 1, "state": "fault", "fault": "hardware",
                       "outcome": "cancelled", "durable": true, "motor": 0}),
            ),
            expect(
                10_000,
                json!({"boots": 2, "state": "idle", "pending": false}),
            ),
        ],
        10_100,
        "single",
    );

    let stopped = stopped_at(&report, 2200);
    assert!((2500..=2500 + 10).contains(&stopped), "{stopped}");
}

/// The `stop-fail` variant's long hold: no stop takes. Only the watchdog
/// ends the drive: the owner's beat lapses 1 s after its last, and the
/// expiry follows. The next boot resolves the feed as uncertain.
#[test]
fn a_stop_that_never_takes_ends_in_the_watchdog_reset() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 2]),
            act(2200, "stop_fails", &[1]),
            act(2200, "cancel", &[1]),
            expect(3000, json!({"boots": 1, "motor": 1, "durable": false})),
            expect(
                10_000,
                json!({"boots": 2, "state": "terminal", "outcome": "uncertain",
                       "durable": true, "pending": false, "motor": 0}),
            ),
        ],
        10_100,
        "single",
    );

    let reset = events(&report, "watchdog_reset_requested")
        .map(|e| e["at_ms"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(reset.len(), 1);
    assert!((2200 + 1000 + 2426 - 20..=2200 + 1000 + 5460).contains(&reset[0]));
    assert_eq!(
        stopped_at(&report, 2200),
        reset[0],
        "driving until the reset"
    );
}

#[test]
fn reboot_after_completion_rejects_replay_and_accepts_new_id() {
    passing(
        vec![
            act(SUBMIT, "submit", &[10, 1]),
            expect(ONE_PORTION, json!({"durable": true, "completed": 1})),
            act(ONE_PORTION + 1, "power_cut", &[]),
            expect(ONE_PORTION + 1, json!({"online": false})),
            act(ONE_PORTION + 2, "boot", &[]),
            expect(
                ONE_PORTION + 3,
                json!({"state": "idle", "high_water_id": 10, "pending": false}),
            ),
            act(ONE_PORTION + 10, "submit", &[10, 1]),
            expect(ONE_PORTION + 10, json!({"submission": "reject_id"})),
            act(ONE_PORTION + 11, "submit", &[11, 1]),
            expect(ONE_PORTION + 11, json!({"submission": "enqueued"})),
            expect(
                2 * ONE_PORTION + 100,
                json!({"durable": true, "completed": 1, "id": 11}),
            ),
        ],
        2 * ONE_PORTION + 200,
        "single",
    );
}

/// The rotor keeps its place through a power cut: the next feed starts
/// where the cut one stopped, so its first mark comes that much sooner.
#[test]
fn the_rotor_keeps_its_place_through_a_power_cut() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(3000, "power_cut", &[]),
            act(3001, "boot", &[]),
            act(3100, "ack", &[1]),
            act(3200, "submit", &[2, 1]),
            expect(
                3200 + ONE_PORTION,
                json!({"state": "terminal", "outcome": "done", "id": 2, "completed": 1}),
            ),
        ],
        3200 + ONE_PORTION,
        "single",
    );

    let drives: Vec<u64> = events(&report, "outputs")
        .filter(|e| e["motor"] == 1)
        .filter_map(|e| e["at_ms"].as_u64())
        .collect();
    let rise = events(&report, "feedback")
        .find(|e| e["high"] == true)
        .and_then(|e| e["at_ms"].as_u64())
        .unwrap();
    // The first feed drove about 2.78 s; the second meets the rise that
    // much sooner than a feed from rest would.
    let driven = 3000 - drives[0];
    let into = rise - drives[1];
    assert!(into.abs_diff(9037 - driven) <= 100, "{into} after {driven}");
}

#[test]
fn power_cut_before_reservation_does_not_consume_id() {
    passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(SUBMIT, "power_cut", &[]),
            act(SUBMIT + 1, "boot", &[]),
            expect(SUBMIT + 2, json!({"state": "idle", "high_water_id": 0})),
            act(SUBMIT + 10, "submit", &[1, 1]),
            expect(SUBMIT + 10, json!({"submission": "enqueued"})),
            expect(ONE_PORTION + 100, json!({"completed": 1, "durable": true})),
        ],
        ONE_PORTION + 200,
        "single",
    );
}

#[test]
fn power_cut_during_motion_resolves_as_uncertain_and_is_never_retried() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            expect(3000, json!({"state": "feeding", "motor": 1})),
            act(3000, "power_cut", &[]),
            act(3001, "boot", &[]),
            expect(
                3002,
                json!({"state": "terminal", "fault": "ok", "id": 1, "outcome": "uncertain",
                       "completed": 0, "durable": true, "pending": false, "high_water_id": 1}),
            ),
            act(3010, "submit", &[2, 1]),
            expect(3010, json!({"submission": "reject_busy"})),
            act(3011, "ack", &[1]),
            expect(3100, json!({"state": "idle", "high_water_id": 1})),
            act(3101, "submit", &[1, 1]),
            expect(3101, json!({"submission": "reject_id"})),
            expect(20_000, json!({"motor": 0, "optics": 0, "state": "idle"})),
        ],
        20_100,
        "single",
    );

    assert!(!drove_after_reboot(&report));
}

#[test]
fn byte_cuts_at_erase_body_and_commit_boundaries() {
    for phase in [1, 2] {
        for byte in [0, 1, 256, 257, 270, 380, 381, 383, 384] {
            // A reservation cut before its first byte leaves the journal at
            // rest; any later cut of it, a committed one, or one whose result
            // was cut, resolves as uncertain; a committed completion is idle.
            let expected = if phase == 1 && byte <= 256 {
                json!({"state": "idle", "high_water_id": 0})
            } else if phase == 2 && byte == 384 {
                json!({"state": "idle", "high_water_id": 1, "pending": false})
            } else {
                json!({"state": "terminal", "outcome": "uncertain", "durable": true,
                       "pending": false, "high_water_id": 1, "fault": "ok"})
            };

            let mut values = json!({"motor": 0, "optics": 0});
            values
                .as_object_mut()
                .unwrap()
                .extend(expected.as_object().unwrap().clone());

            let report = passing(
                vec![
                    act(0, "cut", &[phase, byte]),
                    act(SUBMIT, "submit", &[1, 1]),
                    expect(ONE_PORTION + 1000, json!({"online": false})),
                    act(ONE_PORTION + 1001, "boot", &[]),
                    expect(ONE_PORTION + 1002, values),
                ],
                ONE_PORTION + 1100,
                "single",
            );

            assert_eq!(events(&report, "power_cut").count(), 1, "{phase} {byte}");
            assert!(!drove_after_reboot(&report), "{phase} {byte}");
        }
    }
}

#[test]
fn explicit_feedback_replaces_the_plant() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(SUBMIT, "jam", &[1]),
            act(6000, "edge", &[0]),
            act(6100, "edge", &[1]),
            expect(
                6200,
                json!({"state": "terminal", "completed": 1, "durable": true}),
            ),
        ],
        6300,
        "single",
    );
    assert_eq!(events(&report, "feedback").count(), 2);
}

#[test]
fn identical_scenarios_have_identical_reports() {
    let data = with_plant(
        vec![
            act(SUBMIT, "submit", &[1, 2]),
            act(SUBMIT, "write", &[3, 4000]),
            expect(2 * ONE_PORTION, json!({"completed": 2})),
        ],
        2 * ONE_PORTION,
        "single",
        json!({"seed": 9, "double_permille": 200}),
    );
    assert_eq!(run(&data).unwrap(), run(&data).unwrap());
}

/// The start-up surge reads over the limit for a poll or two, inside the
/// 150 ms after a change of drive that counts toward no jam; one that
/// outlasted the window would read as a jam both ways.
#[test]
fn the_start_up_surge_is_tolerated_inside_its_window() {
    let feed = vec![
        act(SUBMIT, "submit", &[1, 1]),
        expect(ONE_PORTION, json!({"state": "terminal"})),
    ];

    for surge in [20, 140] {
        let plant = json!({ "surge_ms": surge });
        let report = checked(&with_plant(feed.clone(), ONE_PORTION, "single", plant));
        assert_eq!(last_status(&report)["outcome"], 1, "done with {surge} ms");
    }

    let long = with_plant(feed, ONE_PORTION, "single", json!({"surge_ms": 400}));
    let report = checked(&long);
    let status = last_status(&report);
    assert_eq!(status["outcome"], 4, "failed");
    assert_eq!(status["reason"], 13, "jammed");
    assert_eq!(
        drives(&report, 2),
        1,
        "backed off once, and over the limit both ways"
    );
}

/// How many separate drives `motor` (1 forward, 2 back) the outputs showed.
fn drives(report: &Value, motor: u64) -> usize {
    events(report, "outputs")
        .filter(|e| e["motor"] == motor)
        .count()
}

/// A jam freed just before a mark, and one just past a credited mark: the
/// mark the rotor reaches soon after the resume is credited (no inhibit
/// window restarts there), and the one it recrosses while backing off is
/// not credited twice. Each feed stops at the rise of its last real mark.
#[test]
fn a_jam_next_to_a_mark_credits_each_mark_once() {
    // The drive starts about 220 ms after the submission; the first rise is
    // about 9.07 s of drive in, the second about 18.15 s.
    for (portions, wedge_at, rises) in [(1, 8700, 1), (2, 9600, 2), (2, 18_000, 2)] {
        let report = passing(
            vec![
                act(SUBMIT, "submit", &[1, portions]),
                act(wedge_at, "wedge", &[1]),
                expect(
                    portions * 9100 + 4000,
                    json!({"state": "terminal", "outcome": "done", "completed": portions,
                           "durable": true, "motor": 0}),
                ),
            ],
            portions * 9100 + 4100,
            "single",
        );
        assert_eq!(drives(&report, 2), 1, "wedge at {wedge_at}");
        // The rotor ends just past the last real rise: no mark credited
        // twice, none skipped.
        let rotor = last_status(&report)["rotor_ms"].as_i64().unwrap();
        let expected = 9074 * rises;
        assert!(
            (expected - 100..expected + 100).contains(&rotor),
            "wedge at {wedge_at}: rotor at {rotor}"
        );
    }
}

/// A wedge the first reversal frees: the drive backs off once, then
/// completes the portion, its deadline lengthened by what backing off cost.
#[test]
fn a_jam_the_first_reversal_frees_lets_the_feed_complete() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(4000, "wedge", &[1]),
            expect(
                ONE_PORTION + 3000,
                json!({"state": "terminal", "outcome": "done", "completed": 1, "durable": true,
                       "motor": 0, "optics": 0}),
            ),
        ],
        ONE_PORTION + 3100,
        "single",
    );
    assert_eq!(drives(&report, 2), 1);
    assert_eq!(drives(&report, 1), 2, "forward, then forward again");
}

/// A wedge no reversal frees: three reversals, as stock makes, then the
/// feed ends failed `Jammed`, stopped, and is not started again. The
/// whole drive stays inside the deadline the reversals lengthened.
#[test]
fn a_jam_that_holds_ends_the_feed_after_three_reversals() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            act(3000, "wedge", &[2]),
            expect(
                12_000,
                json!({"state": "terminal", "outcome": "failed", "reason": "jammed",
                       "completed": 0, "durable": true, "motor": 0, "optics": 0}),
            ),
            act(12_001, "ack", &[1]),
            expect(30_000, json!({"state": "idle", "motor": 0})),
        ],
        30_100,
        "single",
    );
    assert_eq!(drives(&report, 2), 3);
    assert_eq!(drives(&report, 1), 4);
}

/// Marks that stop coming (the rotor jammed with no current to show it, or
/// the sensor lost): no reversal, since nothing says which, and the feed
/// ends `FeedbackTimeout` once no edge came for 11 s of drive, before its
/// 12 s deadline.
#[test]
fn lost_marks_end_the_feed_without_a_reversal() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(SUBMIT, "jam", &[1]),
            expect(11_000, json!({"state": "feeding"})),
            expect(
                11_500,
                json!({"state": "terminal", "outcome": "failed", "reason": "feedback_timeout",
                       "durable": true, "motor": 0}),
            ),
        ],
        11_600,
        "single",
    );
    assert_eq!(drives(&report, 2), 0);
}

/// The mark input held LOW past any mark (4 s; a mark reads LOW for about
/// 1.2 s) is a stuck mark, not a portion.
#[test]
fn a_mark_input_stuck_low_is_a_stuck_mark() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            act(SUBMIT, "jam", &[1]),
            act(1000, "edge", &[0]),
            expect(4900, json!({"state": "feeding"})),
            expect(
                5100,
                json!({"state": "terminal", "outcome": "failed", "reason": "mark_stuck",
                       "completed": 0, "durable": true, "motor": 0}),
            ),
        ],
        5200,
        "single",
    );
    assert_eq!(drives(&report, 2), 0);
}

/// An outlet blocked before the feed: the motor never starts, and the feed
/// fails naming the outlet (stock ends such a feed too).
#[test]
fn a_blocked_outlet_fails_the_feed_before_any_motion() {
    let report = passing(
        vec![
            act(50, "inputs", &[4]),
            act(SUBMIT, "submit", &[1, 1]),
            expect(
                2500,
                json!({"state": "terminal", "outcome": "failed", "reason": "outlet",
                       "durable": true, "motor": 0, "optics": 0}),
            ),
        ],
        2600,
        "single",
    );
    assert_eq!(drives(&report, 1), 0);
}

/// An outlet that blocks after the first portion ends the feed with that
/// portion: the motor stops, the result names the outlet and counts one,
/// and the outlet clearing again resumes nothing (stock would pause up to
/// about 10 minutes and resume).
#[test]
fn an_outlet_blocked_mid_feed_ends_it_with_the_portions_so_far() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            act(ONE_PORTION + 500, "inputs", &[4]),
            expect(
                ONE_PORTION + 1700,
                json!({"state": "terminal", "outcome": "failed", "reason": "outlet",
                       "completed": 1, "durable": true, "motor": 0}),
            ),
            act(ONE_PORTION + 1800, "inputs", &[5]),
            act(ONE_PORTION + 1801, "ack", &[1]),
            expect(THREE_PORTIONS, json!({"state": "idle", "motor": 0})),
        ],
        THREE_PORTIONS + 100,
        "single",
    );
    assert_eq!(drives(&report, 1), 1, "one drive, never resumed");
}

/// A feed runs whatever the food beam reads, as stock's does: with the
/// beam reading no food it delivers every portion.
#[test]
fn a_feed_on_an_empty_hopper_completes() {
    passing(
        vec![
            act(50, "inputs", &[7]),
            act(SUBMIT, "submit", &[1, 1]),
            expect(
                ONE_PORTION,
                json!({"state": "terminal", "outcome": "done", "completed": 1,
                       "durable": true}),
            ),
        ],
        ONE_PORTION + 100,
        "single",
    );
}

/// Lost sensors are held, not reset on: at rest, or after the feed they
/// ended was recorded, the owner keeps its fault with the outputs stopped
/// and the watchdog fed, refusing feeds until a restart. A reset would only
/// meet the same converter again.
#[test]
fn lost_sensors_are_held_without_a_reset() {
    passing(
        vec![
            act(3000, "adc_stuck", &[1]),
            expect(3100, json!({"state": "fault", "fault": "sensor"})),
            act(4000, "submit", &[1, 1]),
            expect(4000, json!({"submission": "reject_stopped"})),
            expect(
                40_000,
                json!({"state": "fault", "fault": "sensor", "stopped": true, "optics_off": true,
                       "watchdog_resets": 0, "boots": 1, "online": true}),
            ),
        ],
        40_100,
        "single",
    );

    passing(
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            act(3000, "adc_stuck", &[1]),
            expect(
                3200,
                json!({"state": "fault", "fault": "sensor", "outcome": "failed",
                       "reason": "sensor", "durable": true, "motor": 0}),
            ),
            expect(
                40_000,
                json!({"state": "fault", "watchdog_resets": 0, "boots": 1, "motor": 0}),
            ),
        ],
        40_100,
        "single",
    );
}

/// The capture missing an edge never feeds extra: a missed rise leaves its
/// portion uncredited, and the drive then outlasts the feedback timeout.
#[test]
fn a_missed_mark_fails_the_feed_and_never_overfeeds() {
    let report = checked(&with_plant(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            expect(
                THREE_PORTIONS,
                json!({"state": "terminal", "outcome": "failed", "reason": "feedback_timeout",
                       "durable": true, "motor": 0}),
            ),
        ],
        THREE_PORTIONS,
        "single",
        json!({"missed_permille": 1000}),
    ));
    assert_eq!(last_status(&report)["completed"], 0);
    assert!(events(&report, "feedback").all(|e| e["captured"] == false));

    // With some marks missed, every feed still credits at most what it
    // asked for, and stops.
    for seed in 0..8 {
        let report = checked(&with_plant(
            vec![
                act(SUBMIT, "submit", &[1, 3]),
                expect(
                    40_000,
                    json!({"state": "terminal", "durable": true, "motor": 0}),
                ),
            ],
            40_000,
            "single",
            json!({"seed": seed, "missed_permille": 150}),
        ));
        let status = last_status(&report);
        assert!(status["completed"].as_u64().unwrap() <= 3);
        if status["outcome"] != 1 {
            assert_eq!(status["reason"], 6, "seed {seed}: no feedback");
        }
    }
}

/// Bouncing edges never count twice: the inhibit after each credited edge
/// outlasts any bounce, so a feed credits one portion a mark and completes.
#[test]
fn bouncing_edges_credit_one_portion_a_mark() {
    for seed in 0..4 {
        let report = checked(&with_plant(
            vec![
                act(SUBMIT, "submit", &[1, 3]),
                expect(
                    THREE_PORTIONS,
                    json!({"state": "terminal", "outcome": "done", "completed": 3}),
                ),
                act(THREE_PORTIONS + 1, "ack", &[1]),
                act(THREE_PORTIONS + 100, "submit", &[2, 3]),
                expect(
                    2 * THREE_PORTIONS + 100,
                    json!({"state": "terminal", "outcome": "done", "completed": 3, "id": 2}),
                ),
            ],
            2 * THREE_PORTIONS + 100,
            "single",
            json!({"seed": seed, "double_permille": 1000, "period_jitter_ms": 50}),
        ));

        // Each mark crossed passes once; a feed never stops short by more
        // than a mark's LOW (a credit on a fall's bounce).
        let rises = events(&report, "feedback")
            .filter(|e| e["high"] == true)
            .count();
        assert!(rises >= 6, "seed {seed}: {rises}");
    }
}

/// With food the marks come later and stay LOW longer (about 9.8 s apart and
/// 1.38 s LOW on the bench): still inside 12 s a portion and the 15 s
/// feedback timeout.
#[test]
fn a_feed_with_food_completes_inside_the_limits() {
    checked(&with_plant(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            expect(
                THREE_PORTIONS + 3000,
                json!({"state": "terminal", "outcome": "done", "completed": 3}),
            ),
        ],
        THREE_PORTIONS + 3000,
        "single",
        json!({"period_ms": 9820, "low_ms": 1380}),
    ));
}

/// Liveness: wherever power is cut (any journal mutation of a reservation,
/// a result or a resolution, or any moment of a feed), the next boot comes
/// up able to feed: it resolves what the cut left, and a new feed ID is
/// accepted and completes. Nothing is retried: a feed whose ID was resolved
/// never drives again.
#[test]
fn after_any_cut_a_new_feed_id_is_eventually_accepted() {
    // Each cut: the actions up to the last boot, and that boot's time.
    let mut cuts: Vec<(String, Vec<Value>, u64)> = Vec::new();
    // Every mutation near a write's start, its erase's end and its record's
    // end, and a stride through the rest.
    let sample = |total: u64| {
        (0..=total).filter(move |&m| {
            let into = m % 384;
            into <= 4 || into.abs_diff(256) <= 4 || into >= 380 || m % 17 == 0
        })
    };

    // A reservation or a result, at every boundary of its erase and record.
    // A reservation's or a result's write: the erase, then the record.
    let write = (256 + 128) as u64;
    for phase in [1, 2] {
        for mutations in sample(write) {
            cuts.push((
                format!("phase {phase} mutation {mutations}"),
                vec![
                    act(0, "cut", &[phase, mutations]),
                    act(SUBMIT, "submit", &[1, 1]),
                ],
                ONE_PORTION + 1000,
            ));
        }
    }

    // A resolution: of a feed cut mid-drive (one record), and of a
    // reservation cut mid-record (its stand-in, then the result).
    for (first, total) in [
        (act(3000, "power_cut", &[]), write),
        (act(0, "cut", &[1, 300]), 2 * write),
    ] {
        for mutations in sample(total) {
            let mut actions = vec![first.clone(), act(SUBMIT, "submit", &[1, 1])];
            actions.sort_by_key(|action| action["at_ms"].as_u64());
            actions.push(act(4000, "boot", &[]));
            actions.push(act(4000, "cut", &[4, mutations]));
            cuts.push((format!("resolve mutation {mutations}"), actions, 5000));
        }
    }

    // A moment of the feed: queued, preparing, the drive's start, a mark,
    // its rise, the stop, the result's write.
    for at in [
        SUBMIT,
        SUBMIT + 1,
        SUBMIT + 20,
        SUBMIT + 121,
        SUBMIT + 125,
        8200,
        9100,
        9330,
        9340,
        9345,
        9350,
        9360,
    ] {
        cuts.push((
            format!("power cut at {at}"),
            vec![act(SUBMIT, "submit", &[1, 1]), act(at, "power_cut", &[])],
            ONE_PORTION + 1000,
        ));
    }

    let cases: Vec<_> = cuts.into_iter().collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = cases.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for part in cases.chunks(chunk) {
            scope.spawn(move || {
                for (name, actions, boot) in part {
                    a_feed_is_accepted_after(name, actions.clone(), *boot);
                }
            });
        }
    });
}

/// After `actions` (which end with power off, or a boot that cut again),
/// a boot at `boot` reports what the cut left and then takes a new ID.
fn a_feed_is_accepted_after(name: &str, mut actions: Vec<Value>, boot: u64) {
    // Whichever ID is next: a feed the cut resolved is acknowledged first
    // and refuses its own ID; a feed the cut lost takes ID 1 again.
    let next = boot + 100;
    actions.extend([
        act(boot, "boot", &[]),
        act(next, "ack", &[1]),
        act(next + 50, "submit", &[1, 1]),
        act(next + 50, "submit", &[2, 1]),
    ]);
    let end = next + ONE_PORTION + 500;
    actions.push(expect(end, json!({"online": true})));

    let report = checked(&scenario(actions, end, "single"));
    let status = last_status(&report);
    let id = status["id"].as_u64().unwrap();
    assert!(
        status["state"] == 6 && status["outcome"] == 1 && status["durable"] == true,
        "{name}: {status}"
    );
    assert!(
        (1..=2).contains(&id) && status["high_water_id"] == id,
        "{name}: {status}"
    );
    assert_eq!(status["completed"], 1, "{name}");

    // The last boot never faulted, and a resolved ID never drove again.
    let last = events(&report, "power_cut").count() as u64 + 1;
    let after: Vec<&Value> = statuses(&report).filter(|e| e["boot"] == last).collect();
    assert!(after.iter().all(|e| e["fault"] == 0), "{name}");
    if after.iter().any(|e| e["outcome"] == 5) {
        assert_eq!(id, 2, "{name}: an uncertain feed's ID is skipped");
    }
}

/// Liveness: store writes through a feed (the settings writer's stores, the
/// event ring, the update stage) never break it: a write that comes during
/// the feed waits for the outputs' release, and a feed that finds a write
/// queued or running waits for it, then completes; every write lands.
#[test]
fn a_feed_completes_under_background_flash_writes() {
    for (writes, every, start) in [
        // Writes before, during and after a feed.
        (10, 1500, 0),
        // A burst queued just before the feed.
        (5, 0, SUBMIT - 1),
        // One the feed finds holding the flash.
        (1, 0, SUBMIT - 10),
        // Writes through the emitters' settling and the drive's start.
        (10, 25, SUBMIT + 1),
    ] {
        let mut actions = vec![
            act(start, "write", &[writes, every]),
            act(SUBMIT, "submit", &[1, 3]),
            expect(SUBMIT, json!({"submission": "enqueued"})),
        ];
        actions.sort_by_key(|action| action["at_ms"].as_u64());
        let report = passing(
            [
                actions,
                vec![expect(
                    50_000,
                    json!({"state": "terminal", "outcome": "done", "completed": 3,
                           "durable": true, "writes_done": writes, "writes_waiting": 0}),
                )],
            ]
            .concat(),
            50_000,
            "single",
        );

        let leases = events(&report, "lease")
            .filter(|e| e["active"] == true)
            .count() as u64;
        assert_eq!(leases, writes);
    }
}

#[test]
fn invalid_scenarios_rejected_before_worker_launch() {
    let base = scenario(vec![expect(1, json!({"online": true}))], 80, "single");
    let mut bad = Vec::new();

    // Each top-level field invalid.
    for (key, value) in [
        ("version", json!(1)),
        ("end_ms", json!(600_001)),
        ("profile", json!("stock")),
        ("plant", json!(true)),
        ("plant", json!({"low_ms": 3, "low_jitter_ms": 0})),
        ("plant", json!({"teeth": 3})),
        ("feedback_period_ms", json!(10)),
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
        act(1, "run", &[10]),
        json!({"at_ms": 1, "op": "plant", "args": [{}, 0]}),
        act(1, "auto", &[10]),
        act(1, "cut", &[3, 0]),
        act(1, "write", &[0, 10]),
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
        scenario(vec![expect(1, json!({"online": true}))], 2, "single").to_string(),
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
        scenario(vec![expect(1, json!({"completed": 1}))], 2, "single").to_string(),
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

/// The battery channel (raw, default calibration) at rest for the pack
/// scenarios: 1786 scaled, a pack of about 3.68 V, above the floor's clear
/// level (1710). The floor is 1650 (raw 2237) and the cutoff 1550.
const PACK_REST: u64 = 2400;
/// PA16 high (outlet clear) and PA19 low: on the pack alone.
const ON_PACK: u64 = 1;
/// When a one-portion feed on the pack is surely over.
const PACK_FEED_DONE: u64 = SUBMIT + ONE_PORTION;

fn on_pack(plant: Value, actions: Vec<Value>, end: u64) -> Value {
    let mut all = vec![
        act(10, "battery", &[PACK_REST]),
        act(10, "inputs", &[ON_PACK]),
    ];
    all.extend(actions);
    checked(&with_plant(all, end, "single", plant))
}

/// The adapter pulled mid-feed: the feed stops at once with an explicit
/// result (failed, power), is never restarted, and a new feed on the pack
/// then completes.
#[test]
fn adapter_loss_mid_feed_fails_the_feed_and_a_new_one_runs_on_the_pack() {
    let report = passing(
        vec![
            act(SUBMIT, "submit", &[1, 3]),
            expect(3000, json!({"state": "feeding", "motor": 1})),
            act(3000, "inputs", &[ON_PACK]),
            expect(
                3100,
                json!({"state": "terminal", "outcome": "failed", "reason": "power",
                       "motor": 0, "optics": 0, "durable": true, "completed": 0}),
            ),
            expect(8000, json!({"state": "terminal", "motor": 0, "id": 1})),
            act(8000, "ack", &[1]),
            expect(8050, json!({"state": "idle", "supply_low": false})),
            act(8100, "submit", &[2, 1]),
            expect(8100, json!({"submission": "enqueued"})),
            expect(
                8100 + ONE_PORTION,
                json!({"state": "terminal", "id": 2, "outcome": "done", "completed": 1}),
            ),
        ],
        8200 + ONE_PORTION,
        "single",
    );

    // Between the stop and the second feed's start, nothing drove.
    let drove = events(&report, "outputs").any(|e| {
        let at = e["device_ms"].as_u64().unwrap_or(0);
        (3100..8100).contains(&at) && matches!(e["motor"].as_u64(), Some(1 | 2))
    });
    assert!(!drove, "a failed feed is never restarted");
}

/// The pack sags under the motor: a start-up surge under the floor (but
/// over the cutoff) for its 20 ms is tolerated, and the feed completes.
#[test]
fn a_surge_under_the_floor_is_tolerated_on_the_pack() {
    let plant = json!({"surge_sag_adc": 200, "drive_sag_adc": 40});
    let report = on_pack(
        plant,
        vec![
            act(SUBMIT, "submit", &[1, 1]),
            expect(
                PACK_FEED_DONE,
                json!({"state": "terminal", "outcome": "done", "completed": 1}),
            ),
        ],
        PACK_FEED_DONE + 100,
    );
    assert_eq!(last_status(&report)["outcome"], 1, "done");
}

/// A pack that stays under the floor while it drives fails the feed once
/// the hold (300 ms) runs out; one whose surge sags under the cutoff fails
/// it at once.
#[test]
fn a_sustained_sag_or_one_under_the_cutoff_fails_the_feed() {
    for (name, plant, by) in [
        ("held", json!({"drive_sag_adc": 200}), SUBMIT + 800),
        ("cutoff", json!({"surge_sag_adc": 450}), SUBMIT + 300),
    ] {
        let report = on_pack(
            plant,
            vec![
                act(SUBMIT, "submit", &[1, 3]),
                expect(
                    by,
                    json!({"state": "terminal", "outcome": "failed", "reason": "power",
                           "motor": 0, "optics": 0, "durable": true}),
                ),
                expect(by + 1000, json!({"state": "terminal", "motor": 0})),
            ],
            by + 1100,
        );
        let drives = events(&report, "outputs")
            .filter(|e| matches!(e["motor"].as_u64(), Some(1 | 2)))
            .count();
        assert!(drives > 0, "{name}: it started");
        let stopped = events(&report, "outputs")
            .filter(|e| e["device_ms"].as_u64().unwrap_or(0) > by)
            .all(|e| e["motor"] == 0);
        assert!(stopped, "{name}: never restarted");
    }
}

/// The floor at rest on the pack, with its hysteresis: under the floor no
/// feed drives (it ends `Failed`/`Power`); recovered into the band between
/// the floor and the clear level, still none; at the clear level the next
/// one runs.
#[test]
fn the_floor_refuses_feeds_until_the_pack_reads_the_clear_level() {
    let report = on_pack(
        json!({}),
        vec![
            act(100, "battery", &[2200]),
            expect(500, json!({"supply_low": true})),
            act(1000, "submit", &[1, 1]),
            expect(
                3100,
                json!({"state": "terminal", "outcome": "failed", "reason": "power", "motor": 0, "durable": true}),
            ),
            act(3100, "ack", &[1]),
            act(3200, "battery", &[2297]),
            expect(3600, json!({"state": "idle", "supply_low": true})),
            act(3600, "submit", &[2, 1]),
            expect(
                5700,
                json!({"state": "terminal", "id": 2, "outcome": "failed", "reason": "power"}),
            ),
            act(5700, "ack", &[2]),
            act(5800, "battery", &[2309]),
            expect(5850, json!({"state": "idle", "supply_low": false})),
            act(5900, "submit", &[3, 1]),
            expect(
                5900 + ONE_PORTION,
                json!({"state": "terminal", "id": 3, "outcome": "done", "completed": 1}),
            ),
        ],
        6000 + ONE_PORTION,
    );
    let drove_low = events(&report, "outputs").any(|e| {
        e["device_ms"].as_u64().unwrap_or(0) < 5900 && matches!(e["motor"].as_u64(), Some(1 | 2))
    });
    assert!(!drove_low, "no drive while the pack was under its floor");
}
