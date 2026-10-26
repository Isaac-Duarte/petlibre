//! The calibration store: saves alternate and reload, a power cut keeps the
//! old or the new weight, a newer schema is read-only, and a record whose
//! weight is outside the limits is not read or written.

use wellfed_core::store::slot::{RECORD_BYTES, crc32};
use wellfed_device_api::mailbox::{Calibration, PORTION_MG_MAX, PORTION_MG_MIN};
use wellfed_feeder::store::calibration::{CalibrationStatus, CalibrationStore};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo, UNIT};

fn open(f: &mut FakeJournalFlash) -> CalibrationStore<FlashIo> {
    let mut s = CalibrationStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

fn weighed(mg: u32) -> Calibration {
    Calibration {
        portion_mg: Some(mg),
    }
}

#[test]
fn nothing_until_saved_then_alternate_and_reload() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert_eq!(s.status, CalibrationStatus::Empty);
    assert_eq!(s.calibration, Calibration::default());

    assert!(s.save(&weighed(9_800)));
    assert!(s.save(&weighed(10_150)));
    let r = open(&mut f);
    assert_eq!(
        (r.status, r.generation, r.slot),
        (CalibrationStatus::Loaded, 2, 1)
    );
    assert_eq!(r.calibration, weighed(10_150));

    // Clearing the weight is a value of its own.
    let mut r = r;
    assert!(r.save(&Calibration::default()));
    assert_eq!(open(&mut f).calibration, Calibration::default());
}

#[test]
fn a_weight_outside_the_limits_is_never_written() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    for mg in [PORTION_MG_MIN - 1, PORTION_MG_MAX + 1] {
        assert!(!s.save(&weighed(mg)), "{mg}");
    }
    assert_eq!(f.mutations, 0);
    for mg in [PORTION_MG_MIN, PORTION_MG_MAX] {
        assert!(s.save(&weighed(mg)), "{mg}");
    }
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    for history in 0..3u32 {
        let old = |n: u32| weighed(5_000 + n);
        let mut probe = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut probe);
        for n in 0..history {
            assert!(s.save(&old(n)));
        }
        let start = probe.mutations;
        assert!(s.save(&weighed(9_000)));
        let span = i64::from(probe.mutations - start);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(UNIT);
            let mut s = open(&mut f);
            for n in 0..history {
                assert!(s.save(&old(n)));
            }
            f.budget = cut;
            let _ = s.save(&weighed(9_000));
            f.budget = -1;

            let mut r = open(&mut f);
            let before = history
                .checked_sub(1)
                .map_or_else(Calibration::default, old);
            assert!(
                r.calibration == before || r.calibration == weighed(9_000),
                "history {history} cut {cut}"
            );
            assert!(r.writable(), "history {history} cut {cut}: {:?}", r.status);
            assert!(r.save(&weighed(7_000)));
            assert_eq!(open(&mut f).calibration, weighed(7_000));
        }
    }
}

/// Rewrite slot 0's record with `change` applied and a valid checksum.
fn rewrite(f: &mut FakeJournalFlash, change: impl FnOnce(&mut [u8])) {
    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    change(&mut record);
    let crc = crc32(&record[..120]);
    record[120..124].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);
}

#[test]
fn a_newer_schema_is_read_only_with_nothing_measured() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert!(s.save(&weighed(9_800)));
    rewrite(&mut f, |record| record[8] = 2);

    let mut r = open(&mut f);
    assert_eq!(
        (r.status, r.calibration),
        (CalibrationStatus::ReadOnly, Calibration::default())
    );
    assert!(!r.save(&weighed(9_900)));
}

#[test]
fn records_that_fail_their_checks_are_not_read() {
    for (name, change) in [
        (
            "under the limit",
            (|r: &mut [u8]| r[20..24].copy_from_slice(&999u32.to_le_bytes())) as fn(&mut [u8]),
        ),
        ("over the limit", |r: &mut [u8]| {
            r[20..24].copy_from_slice(&50_001u32.to_le_bytes());
        }),
        ("padding", |r: &mut [u8]| r[100] = 1),
        ("generation 0", |r: &mut [u8]| r[12..20].fill(0)),
    ] {
        let mut f = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut f);
        assert!(s.save(&weighed(9_800)));
        rewrite(&mut f, change);
        let r = open(&mut f);
        assert_eq!(
            (r.status, r.calibration),
            (CalibrationStatus::Empty, Calibration::default()),
            "{name}"
        );
    }
}
