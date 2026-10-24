//! The event ring: order and numbering across reopens, wrapping the
//! ring, gaps for cursors older than the ring, and a power cut at every
//! mutation of an append and of the erase that makes room.

use petlibre_core::store::events::{BODY_BYTES, EventError, EventLog, RECORD_BYTES, Read};
use petlibre_testkit::journal_flash::{FakeJournalFlash, FlashIo};

/// Small units keep the wrap tests fast: 4 records each.
const UNIT: usize = 4 * RECORD_BYTES;
const UNITS: usize = 4;

fn open(f: &mut FakeJournalFlash) -> EventLog<FlashIo> {
    let mut log = EventLog::closed();
    // SAFETY: the fake flash outlives the log in every test.
    unsafe { log.open(f.io(), UNITS) };
    log
}

fn body(n: u8) -> [u8; BODY_BYTES] {
    [n; BODY_BYTES]
}

/// Every record after `after`, in order, as (seq, first body byte), with a
/// gap as (from, to + 1000).
fn read_all(log: &EventLog<FlashIo>, mut after: Option<u64>) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    loop {
        match log.read_after(after) {
            Read::Record(record) => {
                out.push((record.seq, u64::from(record.body[0])));
                after = Some(record.seq);
            }
            Read::Gap { from, to } => {
                out.push((from, to + 1000));
                after = Some(to);
            }
            Read::End => return out,
        }
    }
}

#[test]
fn records_keep_their_order_and_numbers_across_reopens() {
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open(&mut f);
    assert_eq!((log.last_seq(), log.read_after(None)), (None, Read::End));

    for n in 1..=5 {
        assert_eq!(log.append(Some(1_700_000_000), &body(n)), Ok(u64::from(n)));
    }
    let log = open(&mut f);
    assert_eq!(log.last_seq(), Some(5));
    assert_eq!(
        read_all(&log, None),
        [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]
    );
    assert_eq!(read_all(&log, Some(3)), [(4, 4), (5, 5)]);
    let Read::Record(first) = log.read_after(None) else {
        panic!();
    };
    assert_eq!(first.at, Some(1_700_000_000));

    let mut log = log;
    assert_eq!(log.append(None, &body(6)), Ok(6));
    let Read::Record(sixth) = log.read_after(Some(5)) else {
        panic!();
    };
    assert_eq!(sixth.at, None);
}

#[test]
fn a_full_ring_drops_its_oldest_unit_and_reports_the_gap() {
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open(&mut f);
    // Four units of four: record 17 erases the first unit (1-4).
    for n in 1..=20 {
        log.append(None, &body(n)).unwrap();
    }
    let log = open(&mut f);
    assert_eq!(log.oldest_seq(), Some(5));
    assert_eq!(log.last_seq(), Some(20));
    let read = read_all(&log, Some(2));
    assert_eq!(read[0], (3, 1004), "records 3 and 4 are gone");
    assert_eq!(read[1], (5, 5));
    assert_eq!(read.last(), Some(&(20, 20)));
    assert_eq!(read_all(&log, None).len(), 16);
}

#[test]
fn a_power_cut_at_every_mutation_loses_at_most_the_record_being_written() {
    // Enough appends to cross two erases, cut at every mutation of each.
    for history in [3usize, 4, 7, 12] {
        let mut clean = FakeJournalFlash::with_units(UNIT, UNITS);
        let mut log = open(&mut clean);
        for n in 1..=history {
            log.append(None, &body(n as u8)).unwrap();
        }
        let before = clean.mutations;
        log.append(None, &body(99)).unwrap();
        let span = i64::from(clean.mutations - before);

        for cut in 0..=span {
            let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
            let mut log = open(&mut f);
            for n in 1..=history {
                log.append(None, &body(n as u8)).unwrap();
            }
            f.budget = cut;
            let mut log = open(&mut f);
            let result = log.append(None, &body(99));
            f.budget = -1;

            let mut log = open(&mut f);
            let records = read_all(&log, None);
            let last = records.last().copied();
            let wrote = last == Some((history as u64 + 1, 99));
            assert!(
                result.is_ok() || !wrote || cut == span,
                "history {history} cut {cut}"
            );
            if !wrote {
                assert_eq!(
                    last.map(|(seq, _)| seq),
                    Some(history as u64),
                    "history {history} cut {cut}"
                );
            }

            // The ring keeps working, with the next number in order.
            let next = log.append(None, &body(100)).unwrap();
            assert_eq!(next, log.last_seq().unwrap());
            assert_eq!(read_all(&log, Some(next - 1)), [(next, 100)]);
        }
    }
}

#[test]
fn a_failed_erase_is_retried_and_a_closed_log_refuses() {
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open(&mut f);
    for n in 1..=16 {
        log.append(None, &body(n)).unwrap();
    }
    f.budget = 0;
    let mut log = open(&mut f);
    assert_eq!(log.append(None, &body(17)), Err(EventError::Io));
    f.budget = -1;
    let mut log = open(&mut f);
    assert_eq!(log.append(None, &body(17)), Ok(17));

    let mut closed = EventLog::<FlashIo>::closed();
    assert_eq!(closed.append(None, &body(1)), Err(EventError::Closed));
}
