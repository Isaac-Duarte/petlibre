//! Owner under flash holds from the external authority.

mod common;

use common::owner::*;
use common::*;
use petlibre_amebad::owner::{OwnerFault, OwnerState, SubmitResult};
use petlibre_amebad::sdk::{self, fake};
use petlibre_core::feed_storage::{OwnerOutcome, OwnerRecovery};
use petlibre_core::flash_coord::{FlashCoord, FlashFault, FlashLimits, FlashPhase, FlashReply};

#[test]
fn flash_hold_waits_before_outputs_and_release_allows_new_work() {
    let mut f = boot();
    f.advance(2);
    let refreshes = hw(|h| h.watchdog_refreshes);
    f.hold();

    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    f.advance(3);
    assert!(f.status().state == OwnerState::PREPARING && f.reserve_calls == 1);
    assert!(f.status().fault == OwnerFault::OK && !f.owner.optics_started);
    assert!(
        f.status().flash_phase == FlashPhase::QUIESCING
            && hw(|h| h.watchdog_refreshes) == refreshes
    );
    inactive_outputs();

    f.release_hold();
    f.advance(5);
    assert!(f.status().state == OwnerState::FEEDING && f.reserve_calls == 1);
    assert!(hw(|h| h.watchdog_refreshes) > refreshes);
    f.stop();
    assert_eq!(f.authority.phase, FlashPhase::IDLE); // Global authority outlives the owner.
}

#[test]
fn flash_interruption_latches_failure_even_after_release() {
    for phase in 0..3 {
        let mut f = boot();
        if phase != 0 {
            f.start_feed();
        } else {
            f.advance(2);
            assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
            fake::run_task();
            assert!(f.owner.optics_started && f.status().state == OwnerState::PREPARING);
        }

        let refreshes = hw(|h| h.watchdog_refreshes);
        f.hold();
        hw(|h| h.blocked_motor_write = phase == 2);
        fake::advance(64, true); // Independent ISR must remove active outputs.
        if phase != 2 {
            inactive_outputs();
        }
        fake::run_task();

        if phase == 2 {
            assert!(
                f.status().fault == OwnerFault::FLASH && f.status().state == OwnerState::REPORTING
            );
            assert!(f.authority.fault == FlashFault::OUTPUT && !f.status().stopped);
            assert!(
                f.finish_calls == 0
                    && hw(|h| h.task_live)
                    && sdk::tim2_en() != 0
                    && hw(|h| h.wake_locks) != 0
            );
            hw(|h| h.blocked_motor_write = false);
            f.advance(1);
            inactive_outputs();
        }

        assert!(f.status().fault == OwnerFault::FLASH && f.status().state == OwnerState::FAULT);
        assert!(f.status().result.outcome == OwnerOutcome::FAILED && f.status().result_durable);
        assert!(f.finish_calls == 1 && hw(|h| h.watchdog_refreshes) == refreshes);

        if phase != 2 {
            f.release_hold();
        }

        f.advance(1);
        assert_eq!(f.owner.submit(2, 1), SubmitResult::REJECT_STOPPED);
        assert!(!f.owner.acknowledge(1));
        assert_eq!(hw(|h| h.watchdog_refreshes), refreshes);
        f.stop();
    }
}

#[test]
fn flash_hold_expires_requests_and_protocol_fault_stops_owner() {
    let mut f = boot();
    f.hold();
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    f.advance(20);

    assert!(
        f.status().state == OwnerState::TERMINAL
            && f.status().result.outcome == OwnerOutcome::EXPIRED
    );
    assert!(!f.status().result.motion_possible && hw(|h| h.watchdog_refreshes) == 0);
    assert!(f.status().result_durable);

    f.authority.trip(FlashFault::TRANSPORT);
    f.advance(1);
    assert!(
        f.status().fault == OwnerFault::FLASH && f.status().flash_fault == FlashFault::TRANSPORT
    );
    assert!(f.status().state == OwnerState::FAULT && hw(|h| h.watchdog_refreshes) == 0);
    inactive_outputs();
    f.stop();

    let mut f = boot();
    f.advance(2);
    let refreshes = hw(|h| h.watchdog_refreshes);
    f.hold();
    f.advance(50); // A stalled flash protocol cannot keep the owner watchdog alive.
    assert!(f.status().fault == OwnerFault::WATCHDOG && hw(|h| h.watchdog_refreshes) == refreshes);
    f.stop();
}

#[test]
fn start_requires_an_external_ready_authority() {
    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
    let config = f.config;
    assert!(!f.try_start(&config));

    f.bind_authority();
    f.authority = FlashCoord::new(&FLASH_LIMITS, 17, 0).unwrap();
    let config = f.config;
    assert!(!f.try_start(&config));

    assert_eq!(f.authority.ready(17, 29, true, 0), FlashReply::ACCEPTED);
    f.authority.trip(FlashFault::PEER_LOST);
    assert!(!f.try_start(&config));

    assert!(!hw(|h| h.task_live) && hw(|h| h.watchdog_init_calls) == 0 && hw(|h| h.output[0]) == 0);
}

#[test]
fn flash_lease_budget_must_fit_watchdog_supervision() {
    // A lease using its whole budget no longer faults an idle owner.
    let mut f = boot();
    f.advance(2);
    f.hold();
    f.advance(FLASH_LIMITS.total_ms);
    assert!(f.status().fault == OwnerFault::OK && f.status().state == OwnerState::IDLE);

    let refreshes = hw(|h| h.watchdog_refreshes);
    f.release_hold();
    f.advance(2);
    assert!(f.status().fault == OwnerFault::OK && hw(|h| h.watchdog_refreshes) > refreshes);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    f.stop();

    // An authority whose budget can outlast the progress gap is refused.
    let limits = FlashLimits {
        quiesce_ms: 100,
        park_ms: 100,
        active_ms: 100,
        release_ms: 100,
        total_ms: 300,
    };
    let config = CONFIG;
    assert!(config.valid() && config.lease_fits(&FLASH_LIMITS) && !config.lease_fits(&limits));

    // The bound includes two sample periods and two timer periods of margin.
    let edge = FlashLimits {
        total_ms: 50 - 2 - 4,
        ..FLASH_LIMITS
    };
    assert!(!config.lease_fits(&edge));
    assert!(config.lease_fits(&FlashLimits {
        total_ms: edge.total_ms - 1,
        ..edge
    }));

    fake::reset_hardware();
    let mut f = Box::new(Fixture::new(PHASE_KNOWN));
    f.authority = FlashCoord::new(&limits, 17, 0).unwrap();
    assert_eq!(f.authority.ready(17, 29, true, 0), FlashReply::ACCEPTED);
    f.config.flash = &raw mut f.authority;
    let config = f.config;
    assert!(!f.try_start(&config));
    assert!(!hw(|h| h.task_live) && hw(|h| h.watchdog_init_calls) == 0 && hw(|h| h.output[0]) == 0);
}
