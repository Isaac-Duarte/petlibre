//! The `stop-fail` variant's injected STOP that does not take, over the
//! owner on fake hardware: the stop fails its readback and the owner's
//! existing failure paths take over, and the hold lifts by itself, after
//! which the timer interrupt's retry stops the motor. Built only with the
//! `stop-fail` feature (`make check-rust` runs it).

#[path = "common/mod.rs"]
mod common;

use common::owner::*;
use common::*;
use wellfed_core::feed::storage::OwnerOutcome;
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::inject::{self, Hold, MAX_HOLD_MS};
use wellfed_hal::owner::{OwnerFault, OwnerState};
use wellfed_hal::sdk::fake;

/// The hold is one static for the process, unlike the per-thread fake, so
/// the tests take turns when run as threads of one process.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The drive `start_feed` leaves on the pins: PA13 HIGH, PA14 LOW.
const DRIVING: u32 = 1 << 13;

#[test]
fn a_held_stop_keeps_driving_until_the_hold_lifts_then_the_timer_stops_it() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = boot();
    f.start_feed();
    let beat_before = beats();

    inject::hold_stops(20);
    assert!(f.owner.cancel(1));
    f.step();

    // The cancel's stop did not take: the pins still drive and the owner
    // knows it, holds its result back and stops beating.
    assert_eq!(motor_pins(), DRIVING);
    let s = f.status();
    assert!(s.state == OwnerState::Reporting && !s.stopped && !s.result_durable);
    assert_eq!(f.task().fault, OwnerFault::Hardware);

    // The timer interrupt retries the stop every tick, each held.
    f.advance(10);
    assert_eq!(motor_pins(), DRIVING);
    let held = inject::status();
    assert!(held.armed && !held.lifted && held.skipped >= 5, "{held:?}");
    assert_eq!(beats(), beat_before, "no beat after the failed stop");

    // Past the hold, the next retry takes; the owner then records the
    // cancellation durably and faults, outputs stopped.
    f.advance(15);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(inject::status().lifted && !inject::status().armed);
    let s = f.status();
    assert!(s.state == OwnerState::Fault && s.stopped && s.optics_off && s.result_durable);
    assert_eq!(s.result.outcome, OwnerOutcome::Cancelled);
    assert_eq!(f.saved_result.outcome, OwnerOutcome::Cancelled);
    f.stop();
}

#[test]
fn a_hold_is_bounded_and_leaves_drive_writes_alone() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    fake::reset_hardware();
    inject::hold_stops(u32::MAX);
    fake::advance(MAX_HOLD_MS / 1000 * 32_768 - 1, false);
    assert!(inject::status().armed);
    fake::advance(2, false);
    assert_eq!(
        inject::status(),
        Hold {
            armed: false,
            lifted: true,
            skipped: 0
        }
    );

    // Armed before a feed: its drive starts, and only its stop is held.
    let mut f = boot();
    inject::hold_stops(1000);
    f.start_feed();
    assert_eq!(inject::status().skipped, 0);
    assert!(f.owner.cancel(1));
    f.step();
    assert_eq!(motor_pins(), DRIVING);
    assert!(inject::status().skipped >= 1);
}
