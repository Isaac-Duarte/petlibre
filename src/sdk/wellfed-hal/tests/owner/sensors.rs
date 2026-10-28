//! Lost sensors, inputs that keep a feed from starting, reversals and the
//! food beam, through the owner.
//!
//! Lost sensors (an ADC conversion that never completes) are held, not
//! reset on: the owner keeps its fault with the outputs verified stopped
//! and goes on beating, so the platform stays up and says why it cannot
//! feed. A lost supervisor still lapses the beat into a watchdog reset.

use crate::common::owner::*;
use crate::common::*;
use wellfed_core::feed::controller::FeedReason;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery};
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::owner::{OwnerConfig, OwnerFault, OwnerState, SubmitResult};
use wellfed_hal::plaf103::pins::{FOOD, OUTLET, bit};
use wellfed_hal::sdk::fake;

/// Steps for longer than the rest gap, and the beats published meanwhile.
fn beats_over_the_gap(f: &mut Fixture) -> u32 {
    let before = beats();
    f.advance(3 * CONFIG.idle_gap_ms);
    beats() - before
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

#[test]
fn lost_sensors_at_rest_are_held_with_the_outputs_stopped() {
    let mut f = boot();
    hw(|h| h.stuck = true);

    f.step();
    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.fault == OwnerFault::Sensor);
    assert!(s.stopped && s.optics_off && motor_pins() == MOTOR_MASK);

    // Beating on, past the gap a lapsed beat would reset at.
    assert!(beats_over_the_gap(&mut f) > 0);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::RejectStopped);
    assert_eq!(f.reserve_calls, 0);
    f.stop();
}

#[test]
fn lost_sensors_during_a_feed_fail_it_as_a_sensor_fault_and_are_held() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.stuck = true);

    f.step();
    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.fault == OwnerFault::Sensor);
    assert!(s.result_durable && s.result.outcome == OwnerOutcome::Failed);
    assert_eq!(s.result.reason, FeedReason::Sensor);
    assert!(s.stopped && s.optics_off && f.finish_calls == 1);

    assert!(beats_over_the_gap(&mut f) > 0);
    assert_eq!(f.finish_calls, 1, "recorded once");
    f.stop();
}

#[test]
fn a_lost_supervisor_still_lapses_the_beat() {
    let mut f = boot();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    f.advance(2); // The emitters on, still preparing.

    // The owner misses its lease with the emitters on: the timer cuts them
    // and faults the supervisor, which a restart must clear.
    fake::advance(12 * 32, true);
    f.step();
    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.fault == OwnerFault::Hardware);
    assert!(s.stopped && s.optics_off);
    assert_eq!(beats_over_the_gap(&mut f), 0);
}

#[test]
fn an_outlet_blocked_before_the_drive_fails_the_feed_with_no_motion() {
    let mut f = boot();
    hw(|h| h.input[0] &= !bit(OUTLET));

    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    for _ in 0..=CONFIG.prepare_timeout_ms {
        f.advance(1);
        assert_eq!(motor_pins(), MOTOR_MASK, "never driven");
    }
    let s = f.status();
    assert!(s.state == OwnerState::Terminal && s.fault == OwnerFault::Ok);
    assert!(s.result_durable && !s.result.motion_possible);
    assert_eq!(
        (s.result.outcome, s.result.reason),
        (OwnerOutcome::Failed, FeedReason::Outlet)
    );

    // Nothing latched: with the outlet clear, the next feed runs.
    hw(|h| h.input[0] |= bit(OUTLET));
    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    f.advance(5);
    assert_eq!(f.status().state, OwnerState::Feeding);
    f.stop();
}

#[test]
fn a_feed_that_only_ran_out_of_time_still_expires() {
    let mut f = boot();
    f.reserve_delay_ticks = 25 * 32;

    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    let s = f.status();
    assert_eq!(
        (s.result.outcome, s.result.reason),
        (OwnerOutcome::Expired, FeedReason::TotalTimeout)
    );
    f.stop();
}

#[test]
fn a_jam_backed_off_is_traced_and_the_feed_completes() {
    let mut f = boot_with(|c| c.feed.max_reverse_attempts = 1);
    f.start_feed();

    // About 375 scaled, over the 300 limit, for two polls; then the pins
    // go back (PA14 alone), stop, and forward again (PA13 alone).
    hw(|h| h.current_adc = 700);
    f.advance(2);
    hw(|h| h.current_adc = 500);
    let mut pins = Vec::new();
    for _ in 0..12 {
        f.advance(1);
        if pins.last() != Some(&motor_pins()) {
            pins.push(motor_pins());
        }
    }
    assert_eq!(pins, [MOTOR_MASK, 1 << 14, MOTOR_MASK, 1 << 13]);

    fake::advance(32, true);
    fake::edge(true);
    f.step();
    let s = f.status();
    assert!(s.state == OwnerState::Terminal && s.result.outcome == OwnerOutcome::Done);
    assert_eq!(s.trace.reversals, 1);
    f.stop();
}

#[test]
fn the_food_beam_reading_outlasts_the_feed_that_took_it() {
    let mut f = boot();
    assert_eq!(f.status().food, None, "the beam is read only lit");

    // The fake's food receiver reads LOW: food in front of the beam.
    f.start_feed();
    assert_eq!(f.status().food, Some(true));
    f.complete();
    let s = f.status();
    assert!(s.state == OwnerState::Terminal && s.optics_off);
    assert_eq!(s.food, Some(true));
    assert_eq!(s.trace.hopper, Some(true), "the feed read only food");
    f.stop();
}

/// A feed runs whatever the food beam reads, as stock's does, so a beam
/// that misreads never skips a meal; its trace keeps that it read empty.
#[test]
fn a_feed_on_an_empty_hopper_completes_and_its_trace_says_it_read_empty() {
    let mut f = boot();
    // The food receiver reads HIGH: no food in front of the beam.
    hw(|h| h.input[0] |= bit(FOOD));

    f.start_feed();
    f.complete();
    let s = f.status();
    assert!(s.state == OwnerState::Terminal && s.result_durable);
    assert_eq!(
        (s.result.outcome, s.result.completed),
        (OwnerOutcome::Done, 1)
    );
    assert_eq!(s.food, Some(false));
    assert_eq!(s.trace.hopper, Some(false));
    f.stop();
}

#[test]
fn an_owner_that_resolved_a_feed_and_lost_its_sensors_holds_the_result() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery {
        high_water_id: 7,
        pending_request: true,
        ..PHASE_KNOWN
    }));
    f.create();
    f.step();
    assert_eq!(f.status().state, OwnerState::Terminal);

    hw(|h| h.stuck = true);
    assert!(f.owner.acknowledge(7));
    f.advance(2);
    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.fault == OwnerFault::Sensor);
    assert!(s.result.id == 7 && s.result_durable && f.resolve_calls == 1);
    assert!(beats_over_the_gap(&mut f) > 0);
    f.stop();
}
