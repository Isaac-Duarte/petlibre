//! The event ring: order and numbering across reopens, wrapping the
//! ring, gaps for cursors older than the ring, the epoch a new ring draws
//! and keeps, and a power cut at every mutation of an append and of the
//! erase that makes room.

use wellfed_core::store::slot::crc32;
use wellfed_feeder::store::events::{
    BODY_BYTES, Batch, EMPTY, EventError, EventLog, RECORD_BYTES, Read,
};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

/// Small units keep the wrap tests fast: 4 records each.
const UNIT: usize = 4 * RECORD_BYTES;
const UNITS: usize = 4;

const EPOCH: u32 = 0x5eed_0001;

fn open(f: &mut FakeJournalFlash) -> EventLog<FlashIo> {
    open_fresh(f, EPOCH)
}

/// Open with `fresh` as the epoch a new ring would draw.
fn open_fresh(f: &mut FakeJournalFlash, fresh: u32) -> EventLog<FlashIo> {
    let mut log = EventLog::closed();
    // SAFETY: the fake flash outlives the log in every test.
    unsafe { log.open(f.io(), UNITS, fresh) };
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

/// As [`read_all`], through batches of `size`: each batch must hold what
/// that many one-record reads would.
fn read_batched(log: &EventLog<FlashIo>, mut after: Option<u64>, size: usize) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut batch = vec![EMPTY; size];
    loop {
        match log.read_batch(after, &mut batch) {
            Batch::Records(len) => {
                assert!((1..=size).contains(&len), "a batch of {len}");
                for record in &batch[..len] {
                    out.push((record.seq, u64::from(record.body[0])));
                    after = Some(record.seq);
                }
            }
            Batch::Gap { from, to } => {
                out.push((from, to + 1000));
                after = Some(to);
            }
            Batch::End => return out,
        }
    }
}

/// What a read after `after` must return, worked out from the raw flash
/// rather than through the log: every slot of every unit decoded, the
/// records of the ring's epoch in units it began, the first record of a
/// number winning, and a gap for a cursor older than the oldest kept.
fn oracle(log: &EventLog<FlashIo>, f: &FakeJournalFlash, after: Option<u64>) -> Vec<(u64, u64)> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let mut kept = std::collections::BTreeMap::new();
    for unit in &f.bytes {
        let mut unit_epoch = None;
        for slot in unit.as_chunks::<RECORD_BYTES>().0 {
            if slot.iter().all(|&b| b == 0xff) {
                break;
            }
            let seq = word(slot, 0);
            if word(slot, 28) != crc32(&slot[..28]) || seq == 0 || seq == u32::MAX {
                continue;
            }
            let epoch = *unit_epoch.get_or_insert(word(slot, 4));
            if word(slot, 4) != epoch || epoch != log.epoch() {
                continue;
            }
            kept.entry(u64::from(seq)).or_insert(u64::from(slot[12]));
        }
    }

    let mut out = Vec::new();
    let Some(oldest) = log.oldest_seq() else {
        return out;
    };
    let want = after.map_or(oldest, |after| after + 1);
    if want < oldest {
        out.push((want, oldest - 1 + 1000));
    }
    out.extend(
        kept.range(want.max(oldest)..)
            .map(|(&seq, &body)| (seq, body)),
    );
    out
}

/// Batches of every size, and single reads, read the ring as the raw flash
/// says they must, from every cursor.
fn batches_match(log: &EventLog<FlashIo>, f: &FakeJournalFlash) {
    let last = log.last_seq().unwrap_or(0);
    for after in std::iter::once(None).chain((0..=last + 1).map(Some)) {
        let expected = oracle(log, f, after);
        assert_eq!(read_all(log, after), expected, "after {after:?}, single");
        for size in [1, 2, 3, 5, 16] {
            assert_eq!(
                read_batched(log, after, size),
                expected,
                "after {after:?}, size {size}"
            );
        }
    }
}

#[test]
fn records_keep_their_order_and_numbers_across_reopens() {
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open(&mut f);
    assert_eq!((log.last_seq(), log.read_after(None)), (None, Read::End));
    assert_eq!(log.read_batch(None, &mut [EMPTY; 4]), Batch::End);

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

/// A batch from the middle of a unit starts at its cursor's slot and stops
/// once full: a few reads, not a scan of the whole unit.
#[test]
fn a_batch_from_mid_unit_reads_only_its_slots() {
    const PER_UNIT: usize = 128;
    let mut f = FakeJournalFlash::with_units(PER_UNIT * RECORD_BYTES, UNITS);
    let mut log = EventLog::closed();
    // SAFETY: the fake flash outlives the log.
    unsafe { log.open(f.io(), UNITS, EPOCH) };
    for n in 0..100 {
        log.append(None, &body(n)).unwrap();
    }

    let before = f.reads;
    let mut batch = [EMPTY; 16];
    assert_eq!(log.read_batch(Some(60), &mut batch), Batch::Records(16));
    let seqs: Vec<u64> = batch.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, (61..=76).collect::<Vec<_>>());
    assert_eq!(f.reads - before, 16, "one read a record");

    // One record after a cursor reads one slot, at the end of the unit
    // too.
    let before = f.reads;
    assert!(matches!(log.read_after(Some(98)), Read::Record(r) if r.seq == 99));
    assert_eq!(f.reads - before, 1);

    // The last record ends a batch short of full without reading on.
    let before = f.reads;
    assert_eq!(log.read_batch(Some(90), &mut batch), Batch::Records(10));
    assert_eq!(f.reads - before, 10);
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
    batches_match(&log, &f);
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
            batches_match(&log, &f);
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

#[test]
fn a_new_ring_takes_its_fresh_epoch_and_keeps_it_across_reopens_and_wraps() {
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open_fresh(&mut f, 7);
    assert_eq!(log.epoch(), 7);
    for n in 1..=20 {
        log.append(None, &body(n)).unwrap();
    }

    // A reopen draws another epoch, but a ring with records keeps its own,
    // through the erase of every unit it started in.
    let mut log = open_fresh(&mut f, 8);
    assert_eq!(log.epoch(), 7);
    for n in 21..=40 {
        log.append(None, &body(n)).unwrap();
    }
    let log = open_fresh(&mut f, 9);
    assert_eq!((log.epoch(), log.last_seq()), (7, Some(40)));
}

#[test]
fn a_re_created_ring_counts_from_one_under_a_new_epoch() {
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open_fresh(&mut f, 7);
    for n in 1..=6 {
        log.append(None, &body(n)).unwrap();
    }

    // A storage wipe: the next open finds nothing and starts again.
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    let mut log = open_fresh(&mut f, 8);
    assert_eq!((log.epoch(), log.last_seq()), (8, None));
    assert_eq!(log.append(None, &body(1)), Ok(1));
}

#[test]
fn a_ring_from_before_epochs_reads_as_epoch_zero() {
    // A record as the ring wrote it with a 64-bit sequence number.
    let mut legacy = [0u8; RECORD_BYTES];
    legacy[..8].copy_from_slice(&1u64.to_le_bytes());
    legacy[12..28].copy_from_slice(&body(1));
    let crc = crc32(&legacy[..28]);
    legacy[28..].copy_from_slice(&crc.to_le_bytes());
    let mut f = FakeJournalFlash::with_units(UNIT, UNITS);
    f.bytes[0][..RECORD_BYTES].copy_from_slice(&legacy);

    let mut log = open_fresh(&mut f, 3);
    assert_eq!((log.epoch(), log.last_seq()), (0, Some(1)));
    assert_eq!(log.append(None, &body(2)), Ok(2));
    let log = open_fresh(&mut f, 4);
    assert_eq!(read_all(&log, None), [(1, 1), (2, 2)]);
    assert_eq!(log.epoch(), 0);
}
