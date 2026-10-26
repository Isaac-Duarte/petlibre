//! The time settings store: saves alternate and reload, a power cut keeps
//! the old or the new settings, a newer schema is read-only, and a record
//! whose fields fail their checks is not read.

use wellfed_core::store::slot::{RECORD_BYTES, crc32};
use wellfed_device_api::mailbox::{NtpServer, TimeSettings, Zone};
use wellfed_feeder::store::time::{TimeStatus, TimeStore};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo, UNIT};

fn open(f: &mut FakeJournalFlash) -> TimeStore<FlashIo> {
    let mut s = TimeStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

fn settings(n: u8) -> TimeSettings {
    TimeSettings {
        zone: Some(Zone::parse(b"MST7MDT,M3.2.0,M11.1.0").unwrap()),
        ntp: NtpServer::Address {
            ip: [10, 42, 103, n],
            port: 123,
        },
    }
}

#[test]
fn defaults_until_saved_then_alternate_and_reload() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert_eq!(s.status, TimeStatus::Empty);
    assert_eq!(
        s.settings,
        TimeSettings {
            zone: None,
            ntp: NtpServer::Gateway { port: 123 },
        }
    );

    assert!(s.save(&settings(1)));
    let off = TimeSettings {
        zone: None,
        ntp: NtpServer::Off,
    };
    assert!(s.save(&off));
    let r = open(&mut f);
    assert_eq!((r.status, r.generation, r.slot), (TimeStatus::Loaded, 2, 1));
    assert_eq!(r.settings, off);

    let mut r = r;
    let gateway = TimeSettings {
        zone: Some(Zone::parse(b"<+0545>-5:45").unwrap()),
        ntp: NtpServer::Gateway { port: 1123 },
    };
    assert!(r.save(&gateway));
    assert_eq!(open(&mut f).settings, gateway);
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    for history in 0..3u8 {
        let mut probe = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut probe);
        for n in 0..history {
            assert!(s.save(&settings(n)));
        }
        let start = probe.mutations;
        assert!(s.save(&settings(9)));
        let span = i64::from(probe.mutations - start);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(UNIT);
            let mut s = open(&mut f);
            for n in 0..history {
                assert!(s.save(&settings(n)));
            }
            f.budget = cut;
            let _ = s.save(&settings(9));
            f.budget = -1;

            let mut r = open(&mut f);
            let old = history
                .checked_sub(1)
                .map_or_else(TimeSettings::default, settings);
            assert!(
                r.settings == old || r.settings == settings(9),
                "history {history} cut {cut}"
            );
            assert!(r.writable(), "history {history} cut {cut}: {:?}", r.status);
            assert!(r.save(&settings(7)));
            assert_eq!(open(&mut f).settings, settings(7));
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
fn a_newer_schema_is_read_only_with_the_defaults() {
    let mut f = FakeJournalFlash::with_unit(UNIT);
    let mut s = open(&mut f);
    assert!(s.save(&settings(1)));
    rewrite(&mut f, |record| record[8] = 2);

    let mut r = open(&mut f);
    assert_eq!(
        (r.status, r.settings),
        (TimeStatus::ReadOnly, TimeSettings::default())
    );
    assert!(!r.save(&settings(2)));
}

#[test]
fn records_that_fail_their_checks_are_not_read() {
    for (name, change) in [
        (
            "unknown server kind",
            (|r: &mut [u8]| r[20] = 3) as fn(&mut [u8]),
        ),
        ("address with port 0", |r: &mut [u8]| r[25..27].fill(0)),
        ("zone longer than kept", |r: &mut [u8]| r[27] = 49),
        ("not a rule", |r: &mut [u8]| r[28] = b'1'),
        ("padding", |r: &mut [u8]| r[100] = 1),
    ] {
        let mut f = FakeJournalFlash::with_unit(UNIT);
        let mut s = open(&mut f);
        assert!(s.save(&settings(1)));
        rewrite(&mut f, change);
        let r = open(&mut f);
        assert_eq!(
            (r.status, r.settings),
            (TimeStatus::Empty, TimeSettings::default()),
            "{name}"
        );
    }
}
