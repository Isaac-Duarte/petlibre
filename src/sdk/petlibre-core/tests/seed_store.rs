//! The stored seed: every boot starts from a new stored seed, a cut save
//! keeps the old or the new seed and stays writable, and a newer schema is
//! read-only.

use petlibre_core::store::seed::{self, SeedStatus, SeedStore};
use petlibre_core::store::slot::{RECORD_BYTES, crc32};
use petlibre_testkit::journal_flash::{FakeJournalFlash, UNIT};

fn open(f: &mut FakeJournalFlash) -> SeedStore {
    let mut s = SeedStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

#[test]
fn each_boot_stores_a_new_seed_and_never_repeats_a_stream() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let s = open(&mut f);
    assert_eq!((s.status, s.seed()), (SeedStatus::Empty, None));

    // The same fresh input every boot, as a broken source might give.
    let fresh = [7; 32];
    let mut outputs = Vec::new();
    let mut seeds = Vec::new();
    for boot in 1..=4u64 {
        let mut s = open(&mut f);
        let mut drbg = seed::start(&mut s, &fresh).unwrap();
        assert_eq!(s.generation, boot);
        seeds.push(*s.seed().unwrap());
        outputs.push(drbg.bytes().unwrap());
    }
    for (i, a) in outputs.iter().enumerate() {
        assert!(outputs[i + 1..].iter().all(|b| b != a), "boot {i} repeated");
        assert!(!seeds.contains(a), "output {i} equals a stored seed");
    }

    let r = open(&mut f);
    assert_eq!((r.status, r.generation, r.slot), (SeedStatus::Loaded, 4, 1));
    assert_eq!(r.seed(), seeds.last());
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    for history in 0..3u8 {
        let saved = |f: &mut FakeJournalFlash| {
            let mut s = open(f);
            for n in 0..history {
                assert!(s.save(&[n; 32]));
            }
            s
        };
        let mut probe = FakeJournalFlash::with_unit(UNIT);
        let mut s = saved(&mut probe);
        let start = probe.mutations;
        assert!(s.save(&[9; 32]));
        let span = i64::from(probe.mutations - start);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(UNIT);
            let mut s = saved(&mut f);
            f.budget = cut;
            let _ = s.save(&[9; 32]);
            f.budget = -1;

            let mut r = open(&mut f);
            let old = history.checked_sub(1).map(|n| [n; 32]);
            assert!(
                r.seed().copied() == old || r.seed() == Some(&[9; 32]),
                "history {history} cut {cut}"
            );
            assert!(r.writable(), "history {history} cut {cut}: {:?}", r.status);
            assert!(r.save(&[5; 32]));
            assert_eq!(open(&mut f).seed(), Some(&[5; 32]));
        }
    }
}

#[test]
fn a_failed_save_still_starts_the_generator() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    f.fail_call = f.calls + 1;
    let mut drbg = seed::start(&mut s, &[3; 32]).unwrap();
    drbg.bytes().unwrap();
    assert_eq!(s.status, SeedStatus::Io);
}

#[test]
fn a_newer_schema_is_read_only() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert!(s.save(&[1; 32]));

    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    record[8] = 2;
    let crc = crc32(&record[..120]);
    record[120..124].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);

    let mut r = open(&mut f);
    assert_eq!((r.status, r.seed()), (SeedStatus::ReadOnly, None));
    assert!(!r.save(&[2; 32]));
}
