//! Owner over the NOR feed journal: durable results across reboots, and a
//! reservation a reset interrupted resolved as uncertain at the next boot.

use crate::common::motor_pins;
use crate::common::owner::*;
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery};
use wellfed_core::store::feed_journal::{JournalConfig, JournalError};
use wellfed_core::store::slot::RECORD_BYTES;
use wellfed_hal::board::MOTOR_MASK;
use wellfed_hal::owner::{OwnerFault, OwnerState, SubmitResult};
use wellfed_hal::sdk::fake;
use wellfed_testkit::journal_flash::UNIT;

#[test]
fn journal_success_and_reboot_preserve_ids() {
    let mut f = boot_with_journal(false);
    f.start_feed();
    assert!(f.journal.view.recovery.pending_request && f.journal.view.recovery.high_water_id == 1);

    f.complete();
    assert!(f.status().state == OwnerState::Terminal && f.status().result_durable);
    assert!(f.journal.view.has_result && f.journal.view.result.outcome == OwnerOutcome::Done);
    assert_eq!(f.journal.view.result.completed, 1);
    f.stop();

    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::Idle && f.status().high_water_id == 1);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::RejectId);

    assert_eq!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    assert!(f.owner.cancel(2));
    f.step();
    assert!(f.status().result_durable && f.journal.view.result.id == 2);
    assert!(
        f.journal.view.result.outcome == OwnerOutcome::Cancelled
            && !f.status().result.motion_possible
    );
    f.stop();

    let mut f = boot_with_journal(true);
    f.start_feed();
    f.complete();
    assert!(f.status().state == OwnerState::Terminal && f.status().result_durable);
    let r = f.journal.view.recovery;
    assert!(r.direction == Direction::D0 && r.credit_next_event && r.phase_known);

    assert!(f.owner.acknowledge(1));
    f.advance(1);
    assert_eq!(f.owner.submit(2, 1), SubmitResult::Enqueued);
    f.advance(6);
    assert_eq!(f.status().state, OwnerState::Feeding);

    assert!(f.owner.cancel(2));
    f.step();
    assert!(f.status().result_durable && !f.journal.view.recovery.phase_known);
    f.stop();

    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::Idle && !f.status().phase_known);
    assert_eq!(f.owner.submit(3, 1), SubmitResult::RejectPhase);
    f.stop();
}

#[test]
fn a_torn_result_resolves_as_uncertain_at_the_next_boot() {
    let mut f = boot_with_journal(false);
    f.start_feed();
    f.flash.budget = (UNIT + 40) as i64; // Erase old snapshot, then tear body.
    f.complete();
    assert!(f.status().state == OwnerState::Fault && f.status().fault == OwnerFault::Storage);
    assert!(!f.status().result_durable && f.status().stopped && f.status().optics_off);

    let writes = f.flash.mutations;
    f.advance(2);
    assert!(f.flash.mutations == writes && f.finish_calls == 1);
    f.stop();

    restart_with_journal(&mut f);
    assert_uncertain(&f, 1);
    assert!(f.journal.view.has_result && f.journal.view.result.outcome == OwnerOutcome::Uncertain);
    f.stop();

    // A resolved reservation stays resolved.
    restart_with_journal(&mut f);
    assert!(f.status().state == OwnerState::Idle && f.resolve_calls == 1);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::RejectId);
    f.stop();

    let mut f = boot_with_journal(false);
    // The final commit reaches media, but its callback reports failure: the
    // drive never started, but the journal holds the reservation.
    f.flash.budget = (UNIT + RECORD_BYTES) as i64;
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    assert!(f.status().state == OwnerState::Fault && !f.status().result.motion_possible);
    assert!(f.status().stopped && f.status().optics_off && f.finish_calls == 0);

    f.stop();
    restart_with_journal(&mut f);
    assert_uncertain(&f, 1);
    f.stop();
}

/// The owner just opened on a reservation an earlier boot left pending.
fn assert_uncertain(f: &Fixture, id: u64) {
    let s = f.status();
    assert!(
        s.state == OwnerState::Terminal && s.fault == OwnerFault::Ok,
        "{s:?}"
    );
    assert!(s.result.id == id && s.result.outcome == OwnerOutcome::Uncertain);
    assert!(s.result_durable && s.high_water_id == id && s.stopped && s.optics_off);
    assert!(!f.journal.view.recovery.pending_request);
}

/// A boot over `flash` that `run` ends as a reset does, on a thread of its
/// own: the owner and the fake hardware are the thread's, so the next boot
/// starts with neither. Power is cut after `budget` journal mutations from
/// the owner's open (never, when negative). The flash it leaves, the
/// mutations the boot made, and what `run` returned.
fn boot_on<R: Send + 'static>(
    flash: Vec<Vec<u8>>,
    dual: bool,
    budget: i64,
    run: impl FnOnce(&mut Fixture) -> R + Send + 'static,
) -> (Vec<Vec<u8>>, i64, R) {
    std::thread::spawn(move || {
        fake::reset_hardware();
        let mut f = Box::new(Fixture::new(OwnerRecovery::default()));
        f.use_journal = true;
        if dual {
            f.config.bowl_mode = BowlMode::Double;
        }
        if !flash.is_empty() {
            f.flash.bytes = flash;
        }

        // SAFETY: the boxed fixture keeps the fake flash in place.
        let io = unsafe { f.flash.io() };
        let c = JournalConfig {
            binding: 42,
            mode: f.config.bowl_mode,
            max_portions: CONFIG.feed.max_portions,
        };
        if f.journal.open(io, c) == JournalError::Empty {
            assert_eq!(
                f.journal.create(io, c, Direction::D0, true, true),
                JournalError::Ok
            );
        }

        let start = f.flash.mutations;
        f.flash.budget = budget;
        f.create();
        f.step();
        let out = run(&mut f);
        (
            f.flash.bytes.clone(),
            i64::from(f.flash.mutations - start),
            out,
        )
    })
    .join()
    .expect("the boot's thread ran to its end")
}

/// Boot 1 of an interrupted feed: a feed of one portion, which the flash
/// budget may cut at any of its journal writes; then the reset.
fn feed(f: &mut Fixture) {
    assert_eq!(f.status().state, OwnerState::Idle);
    assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
    f.step();
    f.advance(5);
    f.complete();
}

/// A feed that is driving when the reset comes.
fn reset_mid_feed(dual: bool) -> Vec<Vec<u8>> {
    let (flash, _, during) = boot_on(Vec::new(), dual, -1, |f| {
        assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
        f.step();
        f.advance(5);
        f.status()
    });
    assert!(during.state == OwnerState::Feeding && during.intent_reserved);
    flash
}

#[test]
fn a_reset_mid_feed_resolves_as_uncertain_and_the_owner_opens() {
    for dual in [false, true] {
        let flash = reset_mid_feed(dual);

        // The next boot reports the feed as uncertain, once, then feeds anew.
        let (flash, _, ()) = boot_on(flash, dual, -1, move |f| {
            assert_uncertain(f, 1);
            assert!(f.resolve_calls == 1 && f.reserve_calls == 0);
            assert_eq!(f.journal.view.result.requested, 1);
            assert_eq!(f.journal.view.recovery.phase_known, !dual);
            assert_eq!(motor_pins(), MOTOR_MASK);

            assert_eq!(f.owner.submit(2, 1), SubmitResult::RejectBusy);
            assert!(f.owner.acknowledge(1));
            f.step();
            assert_eq!(f.status().state, OwnerState::Idle);
            assert_eq!(f.owner.submit(1, 1), SubmitResult::RejectId);
            if dual {
                // Interrupted motion leaves a double bowl's phase unknown.
                assert_eq!(f.owner.submit(2, 1), SubmitResult::RejectPhase);
                return;
            }

            assert_eq!(f.owner.submit(2, 1), SubmitResult::Enqueued);
            f.step();
            f.advance(5);
            f.complete();
            let s = f.status();
            assert!(s.result.id == 2 && s.result.outcome == OwnerOutcome::Done);
            assert!(s.result_durable && f.resolve_calls == 1);
        });

        let (_, writes, ()) = boot_on(flash, dual, -1, move |f| {
            let s = f.status();
            assert!(s.state == OwnerState::Idle && f.resolve_calls == 0);
            assert_eq!(s.high_water_id, if dual { 1 } else { 2 });
        });
        assert_eq!(writes, 0);
    }
}

#[test]
fn a_power_cut_at_any_journal_write_of_a_feed_resolves_at_the_next_boot() {
    let (fresh, _, ()) = boot_on(Vec::new(), false, -1, |_| ());
    let (_, total, ()) = boot_on(fresh.clone(), false, -1, feed);
    let (_, reserving, ()) = boot_on(fresh.clone(), false, -1, |f| {
        assert_eq!(f.owner.submit(1, 1), SubmitResult::Enqueued);
        f.step();
        assert!(f.status().intent_reserved);
    });
    assert!(0 < reserving && reserving < total);

    for cut in 0..=total {
        let (flash, _, ()) = boot_on(fresh.clone(), false, cut, feed);
        let (_, _, ()) = boot_on(flash, false, -1, move |f| {
            let s = f.status();
            assert!(s.stopped && s.optics_off && f.reserve_calls == 0);
            if s.fault != OwnerFault::Ok {
                // A torn reservation beside an idle record is ambiguous, and
                // faults closed: nothing says which ID it would have held.
                assert!(cut < reserving && f.journal.error == JournalError::Corrupt);
                assert!(s.state == OwnerState::Fault && s.fault == OwnerFault::Recovery);
                return;
            }
            match s.state {
                // Cut before the reservation committed: the feed never ran.
                OwnerState::Idle if s.high_water_id == 0 => {}
                // Cut after the result committed.
                OwnerState::Idle => {
                    let result = f.journal.view.result;
                    assert!(s.high_water_id == 1 && result.outcome == OwnerOutcome::Done);
                }
                OwnerState::Terminal => assert_uncertain(f, 1),
                state => panic!("cut {cut}: {state:?}"),
            }
        });
    }
}

#[test]
fn a_power_cut_at_any_journal_write_of_the_resolution_resolves_once() {
    for dual in [false, true] {
        let pending = reset_mid_feed(dual);
        let (_, total, ()) = boot_on(pending.clone(), dual, -1, |f| assert_uncertain(f, 1));
        assert!(total > 0);

        for cut in 0..=total {
            let (flash, writes, s) = boot_on(pending.clone(), dual, cut, |f| f.status());
            if writes < total {
                // Cut mid-resolution: nothing reported, no feed taken.
                assert!(
                    s.state == OwnerState::Fault && s.fault == OwnerFault::Recovery,
                    "{s:?}"
                );
                assert!(!s.result_durable && s.stopped && s.optics_off);
            }

            // The boot after it resolves the feed, unless the cut boot
            // already had: one uncertain result either way.
            let resolved = writes == total;
            let (flash, again, ()) = boot_on(flash, dual, -1, move |f| {
                if resolved {
                    assert!(f.status().state == OwnerState::Idle && f.resolve_calls == 0);
                } else {
                    assert_uncertain(f, 1);
                }
                let view = f.journal.view;
                assert!(view.sequence == 3 && view.result.outcome == OwnerOutcome::Uncertain);
            });
            assert_eq!(again == 0, resolved, "cut {cut}");

            let (_, writes, ()) = boot_on(flash, dual, -1, |f| {
                assert!(f.status().state == OwnerState::Idle && f.resolve_calls == 0);
            });
            assert_eq!(writes, 0);
        }
    }
}
