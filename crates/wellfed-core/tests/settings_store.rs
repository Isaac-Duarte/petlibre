//! Tests for the two-slot settings store: alternating saves, value
//! validation, a power cut at every byte, newer-schema and foreign-binding
//! records, torn-slot fallback and the wear limit.
//!
//! Unlike the feed journal, losing the newest change is acceptable: a cut
//! save must reopen with the old or the new settings and stay writable. A
//! newer-schema record reads as defaults and is never overwritten until an
//! explicit reset.

use wellfed_core::feed_progress::BowlMode;
use wellfed_core::settings_store::{
    Settings, SettingsStatus, SettingsStore, SlotState, WEAR_LIMIT,
};
use wellfed_core::storage::{RECORD_BYTES, crc32};
use wellfed_testkit::journal_flash::{FakeJournalFlash, UNIT};

const BINDING: u64 = 0x5046_5731_3502_0001;
const UNITS: [usize; 3] = [UNIT, 4096, 148];

fn open(f: &mut FakeJournalFlash) -> SettingsStore {
    let mut s = SettingsStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io(), BINDING) };
    s
}

fn custom(volume: u8) -> Settings {
    Settings {
        bowl_mode: BowlMode::DOUBLE,
        lights: false,
        sound: true,
        volume,
        utc_offset_minutes: -420,
    }
}

#[test]
fn empty_saves_alternate_and_reload() {
    for unit in UNITS {
        let mut f = FakeJournalFlash::with_unit(unit);
        let mut s = open(&mut f);
        assert_eq!(
            (s.status, s.settings),
            (SettingsStatus::EMPTY, Settings::default())
        );

        for (i, volume) in [10u8, 20, 30].into_iter().enumerate() {
            assert!(s.save(&custom(volume)));
            assert_eq!((s.generation, s.slot), (i as u64 + 1, (i % 2) as u32));

            let r = open(&mut f);
            assert_eq!(
                (r.status, r.settings, r.generation),
                (SettingsStatus::LOADED, custom(volume), i as u64 + 1)
            );
        }
    }
}

#[test]
fn invalid_values_are_refused_without_writing() {
    let mut f = FakeJournalFlash::default();
    let mut s = open(&mut f);
    let before = f.mutations;
    for bad in [
        Settings {
            volume: 101,
            ..Settings::default()
        },
        Settings {
            utc_offset_minutes: 15 * 60,
            ..Settings::default()
        },
        Settings {
            bowl_mode: BowlMode(9),
            ..Settings::default()
        },
    ] {
        assert!(!s.save(&bad));
    }

    assert_eq!(f.mutations, before);
}

/// Cut power at every byte of a save: the store must reopen with the old or
/// the new settings, writable, never anything else.
#[test]
fn power_cut_at_every_byte_keeps_old_or_new() {
    for unit in [UNIT, 148] {
        for history in 0..3u8 {
            let mut probe = FakeJournalFlash::with_unit(unit);
            let mut s = open(&mut probe);
            for v in 0..history {
                assert!(s.save(&custom(v)));
            }

            let start = probe.mutations;
            assert!(s.save(&custom(99)));
            let span = i64::from(probe.mutations - start);

            for cut in 0..=span {
                let mut f = FakeJournalFlash::with_unit(unit);
                let mut s = open(&mut f);
                for v in 0..history {
                    assert!(s.save(&custom(v)));
                }

                f.budget = cut;
                let _ = s.save(&custom(99));
                f.budget = -1;

                let r = open(&mut f);
                let old = if history == 0 {
                    Settings::default()
                } else {
                    custom(history - 1)
                };

                assert!(
                    r.settings == old || r.settings == custom(99),
                    "unit {unit} history {history} cut {cut}: {:?}",
                    r.status
                );
                assert!(
                    matches!(
                        r.status,
                        SettingsStatus::LOADED | SettingsStatus::RECOVERED | SettingsStatus::EMPTY
                    ),
                    "unit {unit} history {history} cut {cut}: {:?}",
                    r.status
                );

                // A save cut after its first mutation leaves the slot it was
                // writing visibly unfinished.
                if cut > 0 && history > 0 && r.settings == old {
                    assert_ne!(
                        r.slots[1 - r.slot as usize],
                        SlotState::VALID,
                        "unit {unit} history {history} cut {cut}"
                    );
                }

                // Still writable afterwards.
                let mut r = r;
                assert!(r.save(&custom(77)));
                assert_eq!(open(&mut f).settings, custom(77));
            }
        }
    }
}

/// A v2 record with a valid CRC: read as defaults, never overwritten until reset.
#[test]
fn newer_schema_is_read_only_until_reset() {
    let mut f = FakeJournalFlash::default();
    let mut s = open(&mut f);
    assert!(s.save(&custom(5)));

    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    record[8] = 2;
    let crc = crc32(&record[..120]).to_le_bytes();
    record[120..124].copy_from_slice(&crc);
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);

    let mut s = open(&mut f);
    assert_eq!(
        (s.status, s.settings),
        (SettingsStatus::FUTURE, Settings::default())
    );

    let before = f.mutations;
    assert!(!s.save(&custom(6)));
    assert_eq!(f.mutations, before);

    assert!(s.reset() && s.status == SettingsStatus::EMPTY);
    assert!(s.save(&custom(6)));
    assert_eq!(open(&mut f).settings, custom(6));
}

#[test]
fn foreign_binding_or_garbage_is_corrupt_and_read_only() {
    let mut f = FakeJournalFlash::default();
    let mut s = SettingsStore::default();
    unsafe { s.open(f.io(), BINDING ^ 1) };
    assert!(s.save(&custom(1)));

    let mut s = open(&mut f);
    assert_eq!(s.status, SettingsStatus::CORRUPT);
    assert!(!s.save(&custom(2)));

    let mut f = FakeJournalFlash::default();
    f.bytes[1][7] = 0x12;
    let s = open(&mut f);
    assert_eq!(s.status, SettingsStatus::CORRUPT);
}

#[test]
fn torn_newer_slot_falls_back_and_reports_recovered() {
    let mut f = FakeJournalFlash::default();
    let mut s = open(&mut f);
    assert!(s.save(&custom(1)) && s.save(&custom(2)));

    // Tear the newer record (slot 1): drop its commit marker.
    f.bytes[1][124..128].fill(0);
    let r = open(&mut f);
    assert_eq!(
        (r.status, r.settings, r.generation),
        (SettingsStatus::RECOVERED, custom(1), 1)
    );
    assert_eq!(r.slots, [SlotState::VALID, SlotState::DIRTY]);

    // An erase cut short leaves old bits behind the record: invalid.
    f.bytes[1].fill(u8::MAX);
    f.bytes[1][200] = 0;
    assert_eq!(open(&mut f).slots, [SlotState::VALID, SlotState::INVALID]);

    f.bytes[1].fill(u8::MAX);
    let r = open(&mut f);
    assert_eq!(
        (r.status, r.slots),
        (
            SettingsStatus::LOADED,
            [SlotState::VALID, SlotState::ERASED]
        )
    );
}

#[test]
fn wear_limit_refuses_saves() {
    let mut f = FakeJournalFlash::default();
    let mut s = open(&mut f);
    assert!(s.save(&custom(1)));
    s.generation = WEAR_LIMIT;
    let before = f.mutations;
    assert!(!s.save(&custom(2)));
    assert_eq!((s.status, f.mutations), (SettingsStatus::WORN, before));
}
