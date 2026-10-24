//! Owner watchdog supervision and latched initialization faults.

use crate::common::owner::*;
use crate::common::*;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery};
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::owner::{OwnerFault, OwnerState, SubmitResult};
use wellfed_hal::sdk::{self, fake};
use wellfed_hal::supervision::watchdog::WatchdogFault;

#[test]
fn watchdog_requires_owner_progress_and_checks_before_motion() {
    let mut f = boot();
    f.advance(3);
    let refreshes = hw(|h| h.watchdog_refreshes);
    assert!(refreshes > 0 && f.status().watchdog_armed && f.status().watchdog_claimed);

    // 51 ms is just past the fixture's 50 ms progress gap.
    fake::advance(51 * 32, true); // Timer IRQ executes, owner does not.
    assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);
    fake::run_task();
    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Watchdog);
    assert!(f.status().watchdog_fault == WatchdogFault::Progress && f.status().stopped);
    assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);

    f.stop();
    assert!(!f.status().watchdog_armed && !f.status().watchdog_claimed);

    let mut f = boot();
    f.reserve_delay_ticks = 51 * 32;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    fake::run_task();

    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Watchdog);
    assert!(
        f.status().result_durable && !f.status().result.motion_possible && f.status().optics_off
    );
    assert!(f.status().result.outcome == OwnerOutcome::Failed && hw(|h| h.watchdog_refreshes) == 0);
    f.stop();

    let mut f = boot();
    f.task().watchdog_progress = u64::MAX;
    f.advance(2);
    assert!(f.status().fault == OwnerFault::Watchdog && f.status().stopped);
    f.stop();

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.recover_delay_ticks = 51 * 32;
    f.create();
    fake::run_task();
    assert!(hw(|h| h.watchdog_init_calls) == 1 && hw(|h| h.watchdog_refreshes) == 0);
    assert!(
        f.status().fault == OwnerFault::Watchdog && f.status().stopped && f.status().optics_off
    );
    f.stop();
}

#[test]
fn watchdog_masks_stalls_and_late_result_never_reenable() {
    let mut f = boot();
    // Masked for longer than the fixture's 126 ms maximum expiry.
    f.reserve_delay_ticks = 130 * 32;
    f.reserve_mask_irqs = true;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    fake::run_task();

    assert!(hw(|h| h.watchdog_reset_requests) == 1 && hw(|h| h.watchdog_refreshes) == 0);
    assert!(f.status().state == OwnerState::Fault && !f.status().result.motion_possible);
    assert!(f.status().stopped && f.status().optics_off);
    f.stop(); // Model records the reset request; a real reset would end execution.

    let mut f = boot();
    f.start_feed();
    f.finish_delay_ticks = 51 * 32;
    f.complete();

    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Watchdog);
    assert!(f.status().stopped && f.status().optics_off && f.status().result_durable);
    assert_eq!(f.status().result.outcome, OwnerOutcome::Done); // Durable physical result remains valid.
    assert_eq!(hw(|h| h.watchdog_refreshes), f.finish_refreshes);
    assert_ne!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    f.stop();
}

#[test]
fn watchdog_changes_stop_motion_and_failed_close_retains_context() {
    let mut f = boot();
    f.start_feed();
    let refreshes = hw(|h| h.watchdog_refreshes);
    hw(|h| h.hs_control &= !sdk::HS_WDTRST_OPT);
    fake::run_task();

    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Watchdog);
    assert!(f.status().watchdog_fault == WatchdogFault::Register && f.status().stopped);
    assert!(
        f.status().result.outcome == OwnerOutcome::Failed
            && hw(|h| h.watchdog_refreshes) == refreshes
    );
    f.stop();

    let f = boot();
    hw(|h| h.watchdog_disable_blocked = true);
    f.owner.stop();
    fake::run_task();

    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.watchdog_claimed && s.watchdog_armed);
    assert!(s.stopped && s.optics_off && hw(|h| h.task_live) && hw(|h| h.wake_locks) != 0);
    hw(|h| h.watchdog_disable_blocked = false);
    fake::run_task();
    assert!(f.status().state == OwnerState::Stopped && !f.status().watchdog_claimed);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0);
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
            hw(|h| h.efuse_fail = true); // SETUP after the watchdog is armed.
        }

        f.create();
        fake::run_task();
        let fault = if case == 0 {
            OwnerFault::Recovery
        } else {
            OwnerFault::Setup
        };

        assert!(f.status().state == OwnerState::Fault && f.status().fault == fault);
        f.advance(400); // Several hardware watchdog periods.
        let s = f.status();
        assert!(s.state == OwnerState::Fault && s.fault == fault && s.stopped && s.optics_off);
        assert!(s.watchdog_armed && s.watchdog_claimed && s.watchdog_fault == WatchdogFault::Ok);
        assert!(hw(|h| h.watchdog_refreshes) > 0 && hw(|h| h.watchdog_reset_requests) == 0);

        // Latched: no request is taken and the pending feed is never retried.
        assert_eq!(f.owner.submit(8, 1), SubmitResult::RejectStopped);
        assert!(f.reserve_calls == 0 && f.finish_calls == 0);
        assert_eq!(f.recover_calls, u32::from(case == 0));

        // Losing verified STOP readback ends the refreshes, so the watchdog resets.
        hw(|h| {
            h.readback_override_mask[0] = MOTOR_MASK;
            h.readback_override[0] = 0;
        });

        let refreshes = hw(|h| h.watchdog_refreshes);
        f.advance(200);
        assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);
        assert_eq!(hw(|h| h.watchdog_reset_requests), 1);
        hw(|h| h.readback_override_mask[0] = 0);
        f.stop();
    }

    // A hang during recovery still expires the progress deadline: no refresh.
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery {
        pending_request: true,
        ..OwnerRecovery::default()
    }));
    f.recover_delay_ticks = 51 * 32;
    f.create();
    fake::run_task();

    f.advance(200);
    assert!(f.status().fault == OwnerFault::Recovery && f.status().stopped);
    assert!(f.status().watchdog_fault == WatchdogFault::Progress);
    assert!(hw(|h| h.watchdog_refreshes) == 0 && hw(|h| h.watchdog_reset_requests) == 1);
    f.stop();
}
