//! The owner's beat in place of a watchdog of its own, and latched
//! initialization faults.

use crate::common::owner::*;
use crate::common::*;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery};
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::owner::{OwnerFault, OwnerState, SubmitResult};
use wellfed_hal::sdk::fake;
use wellfed_hal::supervision::beat;

#[test]
fn the_beat_requires_owner_progress_and_is_checked_before_motion() {
    let mut f = boot();
    f.advance(3);
    let count = beats();
    let published = beat::read().unwrap();
    assert!(count > 0 && published.gap_ms == 50 && !published.supervised);

    // 51 ms is just past the fixture's 50 ms gap.
    fake::advance(51 * 32, true); // Timer IRQ executes, owner does not.
    assert_eq!(beats(), count);
    f.step();
    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Progress);
    assert!(f.status().stopped && f.task().beat_lost);
    f.advance(5);
    assert_eq!(beats(), count, "a lost beat never resumes");
    f.stop();
    assert!(beat::read().is_none(), "a stopped owner withdraws its beat");

    let mut f = boot();
    f.reserve_delay_ticks = 51 * 32;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();

    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Progress);
    assert!(
        f.status().result_durable && !f.status().result.motion_possible && f.status().optics_off
    );
    assert!(f.status().result.outcome == OwnerOutcome::Failed && beats() == 0);
    f.stop();

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.recover_delay_ticks = 51 * 32;
    f.create();
    f.step();
    assert!(beats() == 0 && f.status().fault == OwnerFault::Progress);
    assert!(f.status().stopped && f.status().optics_off);
    f.stop();
}

#[test]
fn a_feed_beats_supervised_and_a_late_result_never_beats_again() {
    let mut f = boot();
    f.start_feed();
    assert!(
        beat::read().unwrap().supervised,
        "feeding beats as supervised"
    );

    f.finish_delay_ticks = 51 * 32;
    f.complete();

    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Progress);
    assert!(f.status().stopped && f.status().optics_off && f.status().result_durable);
    assert_eq!(f.status().result.outcome, OwnerOutcome::Done); // Durable physical result remains valid.
    assert_eq!(beats(), f.finish_refreshes);
    assert!(f.task().beat_lost);
    assert_ne!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    f.stop();
}

#[test]
fn submit_waits_for_the_first_step() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.create();
    // Opened but never stepped: no supervisor clock has been published.
    assert_eq!(f.owner.submit(1, 1), SubmitResult::RejectBusy);
    f.step();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    assert!(f.status().intent_reserved);
    f.owner.cancel(1);
    f.advance(3);
    f.stop();
}

#[test]
fn determined_initialization_failure_holds_stopped_without_reset_loop() {
    for case in 0..2 {
        fake::reset_hardware();
        let mut f = Box::new(Fixture::new(if case == 0 {
            OwnerRecovery {
                high_water_id: 7,
                pending_request: true, // Power loss mid-feed.
                ..OwnerRecovery::default()
            }
        } else {
            PHASE_KNOWN
        }));
        if case == 1 {
            hw(|h| h.efuse_fail = true); // SETUP after the beat began.
        }

        f.create();
        f.step();
        let fault = if case == 0 {
            OwnerFault::Recovery
        } else {
            OwnerFault::Setup
        };

        assert!(f.status().state == OwnerState::Fault && f.status().fault == fault);
        f.advance(400); // Several gaps.
        let s = f.status();
        assert!(s.state == OwnerState::Fault && s.fault == fault && s.stopped && s.optics_off);
        assert!(beats() > 0 && !f.task().beat_lost);

        // Latched: no request is taken and the pending feed is never retried.
        assert_eq!(f.owner.submit(8, 1), SubmitResult::RejectStopped);
        assert!(f.reserve_calls == 0 && f.finish_calls == 0);
        assert_eq!(f.recover_calls, u32::from(case == 0));

        // Losing verified STOP readback ends the beats, so the runtime resets.
        hw(|h| {
            h.readback_override_mask[0] = MOTOR_MASK;
            h.readback_override[0] = 0;
        });

        let count = beats();
        f.advance(200);
        assert_eq!(beats(), count);
        hw(|h| h.readback_override_mask[0] = 0);
        f.stop();
    }

    // A hang during recovery still expires the gap: no beat, ever.
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery {
        pending_request: true,
        ..OwnerRecovery::default()
    }));
    f.recover_delay_ticks = 51 * 32;
    f.create();
    f.step();

    f.advance(200);
    assert!(f.status().fault == OwnerFault::Recovery && f.status().stopped);
    assert!(beats() == 0 && f.task().beat_lost);
    f.stop();
}
