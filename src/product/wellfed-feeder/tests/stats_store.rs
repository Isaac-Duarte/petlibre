//! The lifetime statistics store: saves alternate and reload, a power cut
//! at any step keeps the old or the new totals and the store writable, a
//! newer schema is read-only with no totals, a record whose padding is
//! written is not read, and the record's bytes are pinned.

use wellfed_core::store::slot::crc32;
use wellfed_feeder::stats::{EARLY_SAVES, RESTART_SETTLED_MS};
use wellfed_feeder::stats::{TOTAL_SECTORS, Totals};
use wellfed_feeder::store::stats::SaveKind::{self, Early, Restart, Settled};
use wellfed_feeder::store::stats::{Budget, RECORD_BYTES, StatsStatus, StatsStore};
use wellfed_proto::api::{Counts, ResetCause};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

fn open(f: &mut FakeJournalFlash) -> StatsStore<FlashIo> {
    let mut s = StatsStore::default();
    // SAFETY: the fake flash outlives the store in every test.
    unsafe { s.open(f.io()) };
    s
}

/// Totals with every field set from `n`, so a misplaced field shows.
fn totals(n: u32) -> Totals {
    let mut totals = Totals::ZERO.plus(
        &Counts {
            uptime_s: 3_600 * n,
            feeds: 10 * n,
            feeds_failed: n,
            portions: 20 * n,
            presses: 3 * n,
            sessions: 5 * n,
            reconnects: 2 * n,
            dhcp_renewals: 4 * n,
            outages: n + 1,
            battery_s: 60 * n,
        },
        &core::array::from_fn::<u32, TOTAL_SECTORS, _>(|sector| {
            n * 100 + u32::try_from(sector).unwrap()
        }),
        7 * n,
    );
    totals.count_boot(ResetCause::PowerOn);
    for _ in 0..n {
        totals.count_boot(ResetCause::Watchdog);
    }
    totals.count_boot(ResetCause::Software);
    totals
}

/// Early saves take a token each and the one that takes the last sets the
/// sticky flag; past it an early save is refused without a write, and each
/// settled save gives one token back, keeping the flag. Both reload.
#[test]
fn early_saves_take_tokens_a_settled_save_gives_one_back() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = open(&mut f);
    for n in 1..=EARLY_SAVES {
        assert!(s.may_save(Early));
        assert!(s.save(&totals(u32::from(n)), Early));
        assert_eq!(
            s.budget,
            Budget {
                early: n,
                missed: n == EARLY_SAVES
            }
        );
    }
    let before = f.mutations;
    assert!(!s.may_save(Early));
    assert!(!s.save(&totals(9), Early));
    assert_eq!(f.mutations, before, "refused without a write");
    assert!(s.writable(), "a refusal is not a fault");

    let r = open(&mut f);
    assert_eq!(
        (r.budget, r.totals),
        (
            Budget {
                early: EARLY_SAVES,
                missed: true
            },
            totals(u32::from(EARLY_SAVES))
        )
    );
    let mut r = r;
    assert!(r.save(&totals(20), Settled));
    let mut r = open(&mut f);
    assert_eq!(
        r.budget,
        Budget {
            early: EARLY_SAVES - 1,
            missed: true
        }
    );
    assert!(r.may_save(Early));
    assert!(r.save(&totals(21), Early));
    assert!(!r.may_save(Early), "one token, taken");

    for _ in 0..u32::from(EARLY_SAVES) + 2 {
        assert!(r.save(&totals(22), Settled));
    }
    assert_eq!(r.budget.early, 0, "held at none in use");
}

/// A restart's save early in a boot spends a token like any early save;
/// once the boot has run `RESTART_SETTLED_MS` it is outside the budget, so
/// an owner's restart keeps its counts with the budget spent.
#[test]
fn a_settled_boots_restart_saves_with_the_budget_spent() {
    assert_eq!(SaveKind::restart(RESTART_SETTLED_MS - 1), Early);
    assert_eq!(SaveKind::restart(RESTART_SETTLED_MS), Restart);

    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = open(&mut f);
    for n in 1..=EARLY_SAVES {
        assert!(s.save(&totals(u32::from(n)), Early));
    }
    let spent = s.budget;
    assert!(!s.may_save(Early));

    assert!(s.may_save(Restart));
    assert!(s.save(&totals(30), Restart));
    assert_eq!(s.budget, spent, "neither takes nor gives back a token");

    let r = open(&mut f);
    assert_eq!((r.budget, r.totals), (spent, totals(30)));
}

#[test]
fn zero_until_saved_then_alternate_and_reload() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = open(&mut f);
    assert_eq!((s.status, s.totals), (StatsStatus::Empty, Totals::ZERO));
    assert_eq!(s.next_slot(), 0);

    assert!(s.save(&totals(1), Settled));
    assert_eq!(s.next_slot(), 1);
    assert!(s.save(&totals(2), Settled));
    let mut r = open(&mut f);
    assert_eq!(
        (r.status, r.generation, r.slot),
        (StatsStatus::Loaded, 2, 1)
    );
    assert_eq!(r.totals, totals(2));
    assert_eq!(r.next_slot(), 0);

    assert!(r.save(&totals(3), Settled));
    assert_eq!(open(&mut f).totals, totals(3));
}

#[test]
fn power_cut_at_every_mutation_keeps_old_or_new_and_stays_writable() {
    for history in 0..3u32 {
        let mut probe = FakeJournalFlash::with_unit(512);
        let mut s = open(&mut probe);
        for n in 0..history {
            assert!(s.save(&totals(n), Settled));
        }
        let start = probe.mutations;
        assert!(s.save(&totals(9), Settled));
        let span = i64::from(probe.mutations - start);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_unit(512);
            let mut s = open(&mut f);
            for n in 0..history {
                assert!(s.save(&totals(n), Settled));
            }
            f.budget = cut;
            let saved = s.save(&totals(9), Settled);
            f.budget = -1;
            if !saved {
                assert!(!s.writable(), "a failed save is not retried this boot");
                assert!(!s.save(&totals(8), Settled));
            }

            let mut r = open(&mut f);
            let old = history.checked_sub(1).map_or(Totals::ZERO, totals);
            assert!(
                r.totals == old || r.totals == totals(9),
                "history {history} cut {cut}"
            );
            assert!(r.writable(), "history {history} cut {cut}: {:?}", r.status);
            assert!(r.save(&totals(7), Settled));
            assert_eq!(open(&mut f).totals, totals(7));
        }
    }
}

/// Rewrite slot 0's record with `change` applied and a valid checksum.
fn rewrite(f: &mut FakeJournalFlash, change: impl FnOnce(&mut [u8])) {
    let mut record = f.bytes[0][..RECORD_BYTES].to_vec();
    change(&mut record);
    let crc = crc32(&record[..RECORD_BYTES - 8]);
    record[RECORD_BYTES - 8..RECORD_BYTES - 4].copy_from_slice(&crc.to_le_bytes());
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&record);
}

#[test]
fn a_newer_schema_is_read_only_with_no_totals() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = open(&mut f);
    assert!(s.save(&totals(1), Settled));
    rewrite(&mut f, |record| record[8] = 2);

    let mut r = open(&mut f);
    assert_eq!((r.status, r.totals), (StatsStatus::ReadOnly, Totals::ZERO));
    assert!(!r.writable());
    assert!(!r.save(&totals(2), Settled));
}

#[test]
fn records_that_fail_their_checks_are_not_read() {
    for (name, change) in [
        ("padding", (|r: &mut [u8]| r[278] = 1) as fn(&mut [u8])),
        ("early past the budget", |r: &mut [u8]| {
            r[276] = EARLY_SAVES + 1
        }),
        ("unknown flag", |r: &mut [u8]| r[277] = 2),
        ("generation 0", |r: &mut [u8]| r[12..20].fill(0)),
        ("length", |r: &mut [u8]| r[10] = 128),
    ] {
        let mut f = FakeJournalFlash::with_unit(512);
        let mut s = open(&mut f);
        assert!(s.save(&totals(1), Settled));
        rewrite(&mut f, change);
        let r = open(&mut f);
        assert_eq!(
            (r.status, r.totals),
            (StatsStatus::Empty, Totals::ZERO),
            "{name}"
        );
    }
}

/// `bytes` as lowercase hex, 32 bytes a line.
fn lines(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(32)
        .map(|line| line.iter().map(|b| format!("{b:02x}")).collect())
        .collect()
}

/// The bytes a feeder holds: a change here strands every stored total.
#[test]
fn the_record_is_pinned() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = open(&mut f);
    assert!(s.save(&totals(1), Early));

    let found = lines(&f.bytes[0][..RECORD_BYTES]);
    if found != PINNED {
        let shown: Vec<String> = found.iter().map(|l| format!("    \"{l}\",")).collect();
        panic!("the stats record changed; found:\n{}", shown.join("\n"));
    }
    assert!(f.bytes[0][RECORD_BYTES..].iter().all(|&b| b == u8::MAX));
}

/// Magic, version 1, 288 bytes, generation 1; the ten counts; power-on,
/// watchdog and software resets; bank 1's erases (7); sectors 0-47's
/// erases (100-147); one early save, no flags; padding, checksum and
/// `CMIT`.
const PINNED: &[&str] = &[
    "504c465354413031010020010100000000000000100e00000a00000001000000",
    "1400000003000000050000000200000004000000020000003c00000001000000",
    "0100000001000000000000000000000007000000640000006500000066000000",
    "6700000068000000690000006a0000006b0000006c0000006d0000006e000000",
    "6f00000070000000710000007200000073000000740000007500000076000000",
    "7700000078000000790000007a0000007b0000007c0000007d0000007e000000",
    "7f00000080000000810000008200000083000000840000008500000086000000",
    "8700000088000000890000008a0000008b0000008c0000008d0000008e000000",
    "8f000000900000009100000092000000930000000100000063464848434d4954",
];
