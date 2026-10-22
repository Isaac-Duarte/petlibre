//! Owner lifecycle: requests, reservations, cancellation and stop.

mod common;

use core::ffi::c_void;
use core::ptr;

use common::owner::*;
use common::*;
use wellfed_amebad::owner::{Owner, OwnerConfig, OwnerFault, OwnerState, SubmitResult};
use wellfed_amebad::sdk::{self, fake};
use wellfed_core::feed_progress::BowlMode;
use wellfed_core::feed_storage::{OwnerOutcome, OwnerRecovery};

#[test]
fn serialized_lifecycle_and_durable_success() {
    let mut f = boot();
    assert_eq!(f.owner.submit(0, 1), SubmitResult::REJECT_ID);
    assert_eq!(f.owner.submit(1, 0), SubmitResult::REJECT_REQUEST);

    f.start_feed();
    assert_eq!(f.owner.submit(2, 1), SubmitResult::REJECT_BUSY);
    assert!(!f.owner.cancel(99));

    f.complete();
    let s = f.status();
    assert!(s.state == OwnerState::TERMINAL && s.result_durable && s.stopped && s.optics_off);
    assert!(
        s.result.outcome == OwnerOutcome::DONE
            && s.result.completed == 1
            && s.result.motion_possible
    );
    assert!(f.reserve_calls == 1 && f.finish_calls == 1 && !f.disk.pending_request);

    assert!(f.owner.acknowledge(1));
    f.advance(1);

    assert_eq!(f.owner.submit(1, 1), SubmitResult::REJECT_ID);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    assert!(f.owner.cancel(2));
    fake::run_task();

    assert!(
        f.status().result.outcome == OwnerOutcome::CANCELLED && !f.status().result.motion_possible
    );
    assert!(f.finish_calls == 2 && f.disk.high_water_id == 2);
    assert!(!f.owner.acknowledge(1));
    assert!(f.owner.acknowledge(2));

    f.advance(1);
    assert_eq!(f.status().state, OwnerState::IDLE);
    f.stop();
}

#[test]
fn reservation_failure_cannot_move_outputs_or_retry() {
    let mut f = boot();
    f.reserve_fail = true;

    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::STORAGE);
    assert!(!f.status().result.motion_possible && !f.status().result_durable);
    assert!(f.reserve_calls == 1 && f.finish_calls == 0);

    f.advance(5);
    assert!(f.reserve_calls == 1 && f.status().stopped && f.status().optics_off);
    assert!(!f.owner.acknowledge(1));
    f.stop();
}

#[test]
fn slow_reservation_expires_and_cancellation_during_callback_wins() {
    let mut f = boot();
    f.reserve_delay_ticks = 25 * 32;

    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    assert!(
        f.status().result.outcome == OwnerOutcome::EXPIRED && !f.status().result.motion_possible
    );
    f.stop();

    let mut f = boot();
    f.cancel_in_reserve = true;

    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().result.outcome == OwnerOutcome::CANCELLED && f.status().result_durable);
    assert!(!f.status().result.motion_possible);
    f.stop();
}

#[test]
fn interrupted_recovery_blocks_every_new_request() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery {
        high_water_id: 7,
        pending_request: true,
        ..OwnerRecovery::default()
    }));

    f.create();
    fake::run_task();

    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::RECOVERY);
    assert_eq!(f.owner.submit(8, 1), SubmitResult::REJECT_STOPPED);
    assert!(
        f.reserve_calls == 0 && f.finish_calls == 0 && f.status().stopped && f.status().optics_off
    );
    f.stop();
}

#[test]
fn finish_failure_is_not_reported_as_durable_success() {
    let mut f = boot();
    f.start_feed();
    f.finish_fail = true;
    f.complete();

    assert!(f.status().state == OwnerState::FAULT && !f.status().result_durable);
    assert!(
        f.disk.pending_request
            && f.finish_calls == 1
            && f.status().stopped
            && f.status().optics_off
    );

    f.advance(4);
    assert_eq!(f.finish_calls, 1);
    f.stop();
}

fn cancel_hook(context: *mut c_void) {
    let f = context.cast::<Fixture>();
    assert!(unsafe { (*f).owner.cancel(1) });
}

#[test]
fn cancellation_during_acquisition_precedes_feedback() {
    let mut f = boot();
    f.start_feed();
    f.advance(4);
    fake::advance(32, true);
    fake::edge(true);

    let context: *mut c_void = ptr::from_mut(&mut *f).cast();
    hw(|h| {
        h.adc_hook = Some(cancel_hook);
        h.adc_hook_context = context;
    });

    fake::run_task();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    assert!(
        f.status().result.outcome == OwnerOutcome::CANCELLED && f.status().result.completed == 0
    );

    assert!(f.owner.acknowledge(1));
    f.advance(100); // Healthy owner continues servicing the watchdog.
    assert_eq!(f.status().state, OwnerState::IDLE); // Cancelled edges cannot poison a later epoch.
    f.stop();
}

#[test]
fn slow_adc_and_stalled_owner_stop_through_the_supervisor() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.adc_delay_ticks = 12 * 32);

    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().stopped && f.status().optics_off);
    assert!(f.status().result.outcome == OwnerOutcome::FAILED && f.status().result_durable);
    f.stop();

    let mut f = boot();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    f.advance(2); // IR on, still preparing.

    assert!(!f.status().optics_off);
    fake::advance(12 * 32, true);
    assert!(optics_off());
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().stopped);
    f.stop();
}

#[test]
fn stop_waits_for_verified_outputs_and_retains_context() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_motor_write = true);

    f.owner.stop();
    fake::run_task();
    assert!(hw(|h| h.task_live) && sdk::tim2_en() != 0 && hw(|h| h.wake_locks) != 0);
    assert!(f.status().state == OwnerState::REPORTING && f.finish_calls == 0);

    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && f.status().stopped);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0 && f.finish_calls == 1);
}

#[test]
fn ambiguous_double_phase_is_never_assumed_after_cancel() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.config.bowl_mode = BowlMode::DOUBLE;
    f.create();
    fake::run_task();

    f.start_feed();
    assert!(f.owner.cancel(1));
    fake::run_task();
    assert!(f.status().state == OwnerState::TERMINAL && !f.status().phase_known);

    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::REJECT_PHASE);
    f.stop();
}

#[test]
fn failed_startup_and_invalid_configuration_do_not_claim_live_resources() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.bind_authority();

    assert!(!f.try_start(&OwnerConfig {
        wake_lock_id: 17,
        ..f.config
    }));
    assert!(!f.try_start(&OwnerConfig {
        optics_max_on_ms: 120,
        ..f.config
    }));

    hw(|h| h.task_create_fail = true);
    let config = f.config;
    assert!(!f.try_start(&config));
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0 && hw(|h| h.output[0]) == 0);

    hw(|h| {
        h.task_create_fail = false;
        h.efuse_fail = true;
    });
    f.create();
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.recover_calls == 0);
    f.stop();

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.create();
    f.owner.stop(); // Cancel before the task ever opens hardware.
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && !hw(|h| h.task_live));
    assert!(f.recover_calls == 0 && hw(|h| h.output[0]) == 0 && hw(|h| h.wake_locks) == 0);

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    hw(|h| h.blocked_motor_write = true);

    f.create();
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && !f.status().stopped);

    f.owner.stop();
    fake::run_task();
    assert!(hw(|h| h.task_live) && hw(|h| h.wake_locks) != 0);

    hw(|h| h.blocked_motor_write = false);
    fake::run_task();
    assert!(f.status().state == OwnerState::STOPPED && f.status().stopped && f.status().optics_off);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0);
}

#[test]
fn optical_cleanup_and_failed_completion_cannot_publish_success() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_optics_off = true);

    f.owner.stop();
    fake::run_task();
    assert!(
        f.status().state == OwnerState::REPORTING && f.status().stopped && !f.status().optics_off
    );
    assert!(f.finish_calls == 0 && hw(|h| h.task_live) && hw(|h| h.wake_locks) != 0);

    hw(|h| h.blocked_optics_off = false);
    fake::advance(64, true);
    fake::run_task();
    assert!(
        f.status().state == OwnerState::STOPPED && f.status().optics_off && f.finish_calls == 1
    );

    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_motor_write = true);

    f.complete();
    assert!(
        f.status().state == OwnerState::REPORTING
            && !f.status().result_durable
            && f.finish_calls == 0
    );
    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && f.status().result_durable);
    assert!(f.status().result.completed == 1 && f.status().result.outcome == OwnerOutcome::FAILED);
    f.stop();
}

#[test]
fn stop_during_result_write_and_singleton_ownership() {
    let mut f = boot();
    let mut second = Box::<Owner>::default();
    let storage = f.storage();
    assert!(!unsafe { second.start(&f.config, &storage) });

    f.start_feed();
    f.stop_in_finish = true;
    f.complete();
    assert!(f.status().state == OwnerState::STOPPED && f.status().result_durable);
    assert!(!hw(|h| h.task_live) && hw(|h| h.wake_locks) == 0);
}
