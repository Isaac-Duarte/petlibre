//! Owner lifecycle: requests, reservations, cancellation and stop.

use core::ffi::c_void;
use core::ptr;

use crate::common::owner::*;
use crate::common::*;
use wellfed_core::feed::progress::BowlMode;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery};
use wellfed_hal::owner::{Owner, OwnerConfig, OwnerFault, OwnerState, SubmitResult};
use wellfed_hal::sdk::{self, fake};

#[test]
fn serialized_lifecycle_and_durable_success() {
    let mut f = boot();
    assert_eq!(f.owner.submit(0, 1), SubmitResult::RejectId);
    assert_eq!(f.owner.submit(1, 0), SubmitResult::RejectRequest);

    f.start_feed();
    assert_eq!(f.owner.submit(2, 1), SubmitResult::RejectBusy);
    assert!(!f.owner.cancel(99));

    f.complete();
    let s = f.status();
    assert!(s.state == OwnerState::Terminal && s.result_durable && s.stopped && s.optics_off);
    assert!(
        s.result.outcome == OwnerOutcome::Done
            && s.result.completed == 1
            && s.result.motion_possible
    );
    assert!(f.reserve_calls == 1 && f.finish_calls == 1 && !f.disk.pending_request);

    assert!(f.owner.acknowledge(1));
    f.advance(1);

    assert_eq!(f.owner.submit(1, 1), SubmitResult::RejectId);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    assert!(f.owner.cancel(2));
    f.step();

    assert!(
        f.status().result.outcome == OwnerOutcome::Cancelled && !f.status().result.motion_possible
    );
    assert!(f.finish_calls == 2 && f.disk.high_water_id == 2);
    assert!(!f.owner.acknowledge(1));
    assert!(f.owner.acknowledge(2));

    f.advance(1);
    assert_eq!(f.status().state, OwnerState::Idle);
    f.stop();
}

#[test]
fn reservation_failure_cannot_move_outputs_or_retry() {
    let mut f = boot();
    f.reserve_fail = true;

    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Storage);
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

    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    assert!(f.status().state == OwnerState::Terminal && f.status().result_durable);
    assert!(
        f.status().result.outcome == OwnerOutcome::Expired && !f.status().result.motion_possible
    );
    f.stop();

    let mut f = boot();
    f.cancel_in_reserve = true;

    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    assert!(f.status().result.outcome == OwnerOutcome::Cancelled && f.status().result_durable);
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
    f.step();

    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Recovery);
    assert_eq!(f.owner.submit(8, 1), SubmitResult::RejectStopped);
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

    assert!(f.status().state == OwnerState::Fault && !f.status().result_durable);
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
    // SAFETY: the fixture keeps the owner and its storage in place while it runs.
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

    f.step();
    assert!(f.status().state == OwnerState::Terminal && f.status().result_durable);
    assert!(
        f.status().result.outcome == OwnerOutcome::Cancelled && f.status().result.completed == 0
    );

    assert!(f.owner.acknowledge(1));
    f.advance(100); // Healthy owner continues servicing the watchdog.
    assert_eq!(f.status().state, OwnerState::Idle); // Cancelled edges cannot poison a later epoch.
    f.stop();
}

#[test]
fn slow_adc_and_stalled_owner_stop_through_the_supervisor() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.adc_delay_ticks = 12 * 32);

    f.step();
    assert!(f.status().state == OwnerState::Fault && f.status().stopped && f.status().optics_off);
    assert!(f.status().result.outcome == OwnerOutcome::Failed && f.status().result_durable);
    f.stop();

    let mut f = boot();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    f.advance(2); // IR on, still preparing.

    assert!(!f.status().optics_off);
    fake::advance(12 * 32, true);
    assert!(optics_off());
    f.step();
    assert!(f.status().state == OwnerState::Fault && f.status().stopped);
    f.stop();
}

#[test]
fn stop_waits_for_verified_outputs_and_retains_context() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_motor_write = true);

    f.owner.stop();
    f.step();
    assert!(f.owner.running() && sdk::tim2_en() != 0 && hw(|h| h.wake_locks) != 0);
    assert!(f.status().state == OwnerState::Reporting && f.finish_calls == 0);

    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    f.step();
    assert!(f.status().state == OwnerState::Stopped && f.status().stopped);
    assert!(!f.owner.running() && hw(|h| h.wake_locks) == 0 && f.finish_calls == 1);
}

#[test]
fn ambiguous_double_phase_is_never_assumed_after_cancel() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.config.bowl_mode = BowlMode::Double;
    f.create();
    f.step();

    f.start_feed();
    assert!(f.owner.cancel(1));
    f.step();
    assert!(f.status().state == OwnerState::Terminal && !f.status().phase_known);

    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::RejectPhase);
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

    assert!(!f.owner.running() && hw(|h| h.wake_locks) == 0 && hw(|h| h.output[0]) == 0);

    hw(|h| h.efuse_fail = true);
    f.create();
    f.step();
    assert!(f.status().state == OwnerState::Fault && f.recover_calls == 0);
    f.stop();

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    f.create();
    f.owner.stop(); // Cancel before the task ever opens hardware.
    f.step();
    assert!(f.status().state == OwnerState::Stopped && !f.owner.running());
    assert!(f.recover_calls == 0 && hw(|h| h.output[0]) == 0 && hw(|h| h.wake_locks) == 0);

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    hw(|h| h.blocked_motor_write = true);

    f.create();
    f.step();
    assert!(f.status().state == OwnerState::Fault && !f.status().stopped);

    f.owner.stop();
    f.step();
    assert!(f.owner.running() && hw(|h| h.wake_locks) != 0);

    hw(|h| h.blocked_motor_write = false);
    f.step();
    assert!(f.status().state == OwnerState::Stopped && f.status().stopped && f.status().optics_off);
    assert!(!f.owner.running() && hw(|h| h.wake_locks) == 0);
}

#[test]
fn optical_cleanup_and_failed_completion_cannot_publish_success() {
    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_optics_off = true);

    f.owner.stop();
    f.step();
    assert!(
        f.status().state == OwnerState::Reporting && f.status().stopped && !f.status().optics_off
    );
    assert!(f.finish_calls == 0 && f.owner.running() && hw(|h| h.wake_locks) != 0);

    hw(|h| h.blocked_optics_off = false);
    fake::advance(64, true);
    f.step();
    assert!(
        f.status().state == OwnerState::Stopped && f.status().optics_off && f.finish_calls == 1
    );

    let mut f = boot();
    f.start_feed();
    hw(|h| h.blocked_motor_write = true);

    f.complete();
    assert!(
        f.status().state == OwnerState::Reporting
            && !f.status().result_durable
            && f.finish_calls == 0
    );
    hw(|h| h.blocked_motor_write = false);
    fake::advance(64, true);
    f.step();
    assert!(f.status().state == OwnerState::Fault && f.status().result_durable);
    assert!(f.status().result.completed == 1 && f.status().result.outcome == OwnerOutcome::Failed);
    f.stop();
}

#[test]
fn stop_during_result_write_and_singleton_ownership() {
    let mut f = boot();
    let second = Box::<Owner<FixtureStorage>>::default();
    let storage = f.storage();
    // SAFETY: the fixture keeps the owner and its storage in place while it runs.
    assert!(!unsafe { second.open(&f.config, storage) });

    f.start_feed();
    f.stop_in_finish = true;
    f.complete();
    assert!(f.status().state == OwnerState::Stopped && f.status().result_durable);
    assert!(!f.owner.running() && hw(|h| h.wake_locks) == 0);
}

#[test]
fn idle_captures_publish_power_health_in_place_of_the_runtime_monitor() {
    wellfed_hal::power::reset();
    let mut f = boot();
    f.advance(3);

    let now = wellfed_hal::sdk::uptime_ms();
    let health = wellfed_hal::power::health(now).expect("the owner's captures publish power");
    assert!(health.adapter && health.battery_mv.is_some());
    f.stop();
}

#[test]
fn a_feed_traces_its_drive_edges_and_current() {
    let mut f = boot();
    f.start_feed();
    f.advance(2);
    fake::edge(false); // Inside the inhibit window: counted, never credited.
    f.complete();

    let s = f.status();
    assert!(s.state == OwnerState::Terminal && s.result.outcome == OwnerOutcome::Done);
    let t = s.trace;
    assert!((t.rising, t.falling) == (1, 1) && t.first_edge_ms.is_some_and(|ms| ms >= 2));
    assert!(t.drive_ms >= t.first_edge_ms.unwrap_or(0) && t.current_samples != 0);
    let logged = [
        t.edge_ms[0].map(|(_, high)| high),
        t.edge_ms[1].map(|(_, high)| high),
    ];
    assert!(logged == [Some(false), Some(true)] && t.edge_ms[2].is_none());
    assert_eq!(t.edge_ms[0].map(|(ms, _)| ms), t.first_edge_ms);
    // The pad watch starts once capture is on, with the drive already applied.
    let start = t.pins.start.unwrap_or_default();
    assert!(start[0] & (1 << 13 | 1 << 14) == 1 << 13 && t.pins.changed == [0, 0]);
    assert!(f.owner.acknowledge(1));
    f.advance(1);

    // A request that never drives reports no drive and none of the old edges.
    assert_eq!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    assert!(f.owner.cancel(2));
    f.step();
    let s = f.status();
    assert!(s.result.outcome == OwnerOutcome::Cancelled && s.trace == Default::default());
    f.stop();
}
