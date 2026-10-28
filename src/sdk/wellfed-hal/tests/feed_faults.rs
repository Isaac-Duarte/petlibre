//! The `feed-jam` and `sensor-fault` variants' injected faults, over the
//! owner on fake hardware: each shows to the board's capture as the real
//! fault would, and the product's own handling ends the feed stopped with
//! its reason. Built only with the `feed-faults` feature (`make check-rust`
//! runs it).

#[path = "common/mod.rs"]
mod common;

use common::owner::*;
use common::*;
use wellfed_core::feed::controller::FeedReason;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::feed_faults::{self, Injected};
use wellfed_hal::owner::{OwnerConfig, OwnerFault, OwnerState, SubmitResult};
use wellfed_hal::sdk::fake;

/// The armed fault is one static for the process, unlike the per-thread
/// fake, so the tests take turns when run as threads of one process.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    feed_faults::arm(Injected::None);
    guard
}

fn boot_with(config: impl FnOnce(&mut OwnerConfig)) -> Box<Fixture> {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    config(&mut f.config);
    f.create();
    f.step();
    assert_eq!(f.status().state, OwnerState::Idle);
    f
}

/// Steps until the owner holds a result or faults, at most `ms`.
fn until_done(f: &mut Fixture, ms: u32) -> OwnerResult {
    for _ in 0..ms {
        let s = f.status();
        if s.result_durable || s.state == OwnerState::Fault {
            break;
        }
        f.advance(1);
    }
    let s = f.status();
    assert!(s.stopped && s.optics_off && motor_pins() == MOTOR_MASK);
    s.result
}

#[test]
fn a_jam_the_first_reversal_frees_lets_the_feed_complete() {
    let _serial = serial();
    let mut f = boot_with(|c| c.feed.max_reverse_attempts = 3);
    f.start_feed();

    feed_faults::arm(Injected::JamFrees);
    f.advance(12);
    assert_eq!(feed_faults::armed(), Injected::None, "backing off freed it");
    assert!(feed_faults::counts().readings >= 2);

    fake::advance(32, true);
    fake::edge(true);
    f.step();
    let s = f.status();
    assert!(s.result.outcome == OwnerOutcome::Done && s.trace.reversals == 1);
    f.stop();
}

#[test]
fn a_jam_that_holds_fails_the_feed_after_every_reversal() {
    let _serial = serial();
    let mut f = boot_with(|c| c.feed.max_reverse_attempts = 3);
    f.start_feed();

    feed_faults::arm(Injected::JamHolds);
    let result = until_done(&mut f, 100);
    assert_eq!(
        (result.outcome, result.reason),
        (OwnerOutcome::Failed, FeedReason::Jammed)
    );
    assert_eq!(f.status().trace.reversals, 3);
    assert_eq!(f.status().fault, OwnerFault::Ok, "a jam is not held");
    feed_faults::arm(Injected::None);
    f.stop();
}

#[test]
fn marks_that_never_come_end_the_feed_on_its_feedback_timeout() {
    let _serial = serial();
    let mut f = boot();
    f.start_feed();

    feed_faults::arm(Injected::MarksMissing);
    f.advance(6);
    // The sensor's rise reaches the pad, but no interrupt comes of it.
    fake::edge(true);
    let result = until_done(&mut f, 100);
    assert_eq!(result.reason, FeedReason::FeedbackTimeout);
    assert_eq!(result.completed, 0);
    assert_eq!(feed_faults::counts().edges, 1);
    feed_faults::arm(Injected::None);
    f.stop();
}

#[test]
fn a_mark_input_stuck_low_is_a_stuck_mark() {
    let _serial = serial();
    let mut f = boot_with(|c| c.feed.mark_low_ms = 10);
    // The pad rests HIGH, as the rotor leaves it between marks.
    fake::edge(true);
    f.start_feed();

    feed_faults::arm(Injected::MarkStuckLow);
    let result = until_done(&mut f, 100);
    assert_eq!(
        (result.outcome, result.reason),
        (OwnerOutcome::Failed, FeedReason::MarkStuck)
    );
    assert_eq!(f.status().fault, OwnerFault::Ok, "a feed's fault, not held");
    feed_faults::arm(Injected::None);
    f.stop();
}

#[test]
fn a_blocked_outlet_fails_the_feed_before_any_motion() {
    let _serial = serial();
    let mut f = boot();

    feed_faults::arm(Injected::OutletBlocked);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    for _ in 0..=CONFIG.prepare_timeout_ms {
        f.advance(1);
        assert_eq!(motor_pins(), MOTOR_MASK);
    }
    let s = f.status();
    assert!(s.result_durable && !s.result.motion_possible);
    assert_eq!(
        (s.result.outcome, s.result.reason),
        (OwnerOutcome::Failed, FeedReason::Outlet)
    );
    feed_faults::arm(Injected::None);
    f.stop();
}

#[test]
fn a_lost_adc_fails_the_feed_and_the_owner_holds_it() {
    let _serial = serial();
    let mut f = boot();
    f.start_feed();

    feed_faults::arm(Injected::AdcFails);
    let result = until_done(&mut f, 10);
    assert_eq!(
        (result.outcome, result.reason),
        (OwnerOutcome::Failed, FeedReason::Sensor)
    );
    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.fault == OwnerFault::Sensor && s.result_durable);

    // Held, beating, with the outputs stopped; clearing the injection does
    // not reopen the board: only a restart does.
    feed_faults::arm(Injected::None);
    let before = beats();
    f.advance(3 * CONFIG.idle_gap_ms);
    assert!(beats() > before);
    assert_eq!(f.status().fault, OwnerFault::Sensor);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::RejectStopped);
    f.stop();
}
