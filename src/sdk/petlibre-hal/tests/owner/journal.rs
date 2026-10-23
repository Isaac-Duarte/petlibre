//! Owner over the NOR feed journal: durable results across reboots.

use crate::common::owner::*;
use petlibre_core::feed::progress::Direction;
use petlibre_core::feed::storage::OwnerOutcome;
use petlibre_core::store::slot::RECORD_BYTES;
use petlibre_hal::owner::{OwnerFault, OwnerState, SubmitResult};
use petlibre_hal::sdk::fake;
use petlibre_testkit::journal_flash::UNIT;

#[test]
fn journal_success_and_reboot_preserve_ids() {
    let mut f = boot_with_journal(false);
    f.start_feed();
    assert!(f.journal.view.recovery.pending_request && f.journal.view.recovery.high_water_id == 1);

    f.complete();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    assert!(f.journal.view.has_result && f.journal.view.result.outcome == OwnerOutcome::DONE);
    assert_eq!(f.journal.view.result.completed, 1);
    f.stop();

    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::IDLE && f.status().high_water_id == 1);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::REJECT_ID);

    assert_eq!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    assert!(f.owner.cancel(2));
    fake::run_task();
    assert!(f.status().result_durable && f.journal.view.result.id == 2);
    assert!(
        f.journal.view.result.outcome == OwnerOutcome::CANCELLED
            && !f.status().result.motion_possible
    );
    f.stop();

    let mut f = boot_with_journal(true);
    f.start_feed();
    f.complete();
    assert!(f.status().state == OwnerState::TERMINAL && f.status().result_durable);
    let r = f.journal.view.recovery;
    assert!(r.direction == Direction::D0 && r.credit_next_event && r.phase_known);

    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    f.advance(6);
    assert_eq!(f.status().state, OwnerState::FEEDING);

    assert!(f.owner.cancel(2));
    fake::run_task();
    assert!(f.status().result_durable && !f.journal.view.recovery.phase_known);
    f.stop();

    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::IDLE && !f.status().phase_known);
    assert_eq!(f.owner.submit(3, 1), SubmitResult::REJECT_PHASE);
    f.stop();
}

#[test]
fn journal_failed_result_and_reservation_block_reboot() {
    let mut f = boot_with_journal(false);
    f.start_feed();
    f.flash.budget = (UNIT + 40) as i64; // Erase old snapshot, then tear body.
    f.complete();
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::STORAGE);
    assert!(!f.status().result_durable && f.status().stopped && f.status().optics_off);

    let writes = f.flash.mutations;
    f.advance(2);
    assert!(f.flash.mutations == writes && f.finish_calls == 1);
    f.stop();

    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::RECOVERY);
    assert!(f.status().high_water_id == 1 && !f.status().phase_known);
    assert_ne!(f.owner.submit(2, 1), SubmitResult::ENQUEUED);
    assert_eq!(f.flash.mutations, writes);
    f.stop();

    let mut f = boot_with_journal(false);
    // The final commit reaches media, but its callback reports failure.
    f.flash.budget = (UNIT + RECORD_BYTES) as i64;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::ENQUEUED);
    fake::run_task();
    assert!(f.status().state == OwnerState::FAULT && !f.status().result.motion_possible);
    assert!(f.status().stopped && f.status().optics_off && f.finish_calls == 0);

    f.stop();
    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::FAULT && f.status().fault == OwnerFault::RECOVERY);
    assert_eq!(f.status().high_water_id, 1);
    f.stop();
}
