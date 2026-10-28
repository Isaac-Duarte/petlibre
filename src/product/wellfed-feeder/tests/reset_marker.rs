//! The factory reset's marker: programmed over a blank or a dirty sector,
//! read back exactly, cleared by an erase, and never pending after a cut
//! part-way through its program or with anything but the whole pattern.

use wellfed_feeder::store::reset::{self, MARKER};
use wellfed_testkit::journal_flash::{FakeJournalFlash, UNIT};

fn one_unit() -> FakeJournalFlash {
    FakeJournalFlash::with_units(UNIT, 1)
}

#[test]
fn a_mark_reads_back_until_cleared() {
    let mut flash = one_unit();
    // SAFETY: the fake flash outlives its I/O in every test.
    let io = unsafe { flash.io() };
    assert!(!reset::pending(&io), "a blank sector is no reset");

    assert!(reset::mark(&io));
    assert!(reset::pending(&io));
    assert!(reset::mark(&io), "marking again keeps it");
    assert!(reset::pending(&io));

    assert!(reset::clear(&io));
    assert!(!reset::pending(&io));
}

#[test]
fn a_dirty_sector_is_no_reset_and_is_erased_before_the_mark() {
    let mut flash = one_unit();
    flash.bytes[0][..8].copy_from_slice(&MARKER[..8]);
    flash.bytes[0][40] = 0;
    // SAFETY: as above.
    let io = unsafe { flash.io() };
    assert!(!reset::pending(&io), "half a pattern is no reset");

    assert!(reset::mark(&io));
    assert!(reset::pending(&io));
}

/// A cut at every mutation of the mark (its erase of a dirty sector, then
/// its program) leaves either no reset or the whole marker, never anything
/// a boot would half-act on.
#[test]
fn a_cut_mark_is_pending_only_once_whole() {
    let mut mutations = None;
    for budget in 0.. {
        let mut flash = one_unit();
        flash.bytes[0][100] = 0x5a;
        flash.budget = budget;
        // SAFETY: as above.
        let io = unsafe { flash.io() };
        let marked = reset::mark(&io);

        flash.budget = -1;
        // SAFETY: as above.
        let io = unsafe { flash.io() };
        let whole = flash.bytes[0][..MARKER.len()] == MARKER;
        assert_eq!(reset::pending(&io), whole, "cut after {budget} mutations");
        assert!(!marked || whole);
        if marked {
            mutations = Some(budget);
            break;
        }
    }
    assert!(mutations.is_some_and(|n| n > 0));
}
