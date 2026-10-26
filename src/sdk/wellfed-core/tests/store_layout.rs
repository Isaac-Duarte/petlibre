//! The on-flash bytes of every record the SDK's stores write, pinned: the
//! feed journal's genesis, reservation and each kind of result (the
//! uncertain resolution among them), the seed, and the update store's
//! record in each of its states. These are the layouts deployed feeders
//! hold, so a change to the shared slot layer must leave every byte where
//! it was. The units are larger than the records so the bytes after each
//! record are pinned erased too.
//!
//! The readers are pinned apart from the writers: each store opens the
//! pinned bytes, placed in erased flash, and reads back the values that
//! wrote them.

use wellfed_core::feed::controller::{FeedReason, FeedRequest};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
use wellfed_core::ota::offer::ImageKind;
use wellfed_core::store::feed_journal::{FeedJournal, JournalConfig, JournalError};
use wellfed_core::store::seed::{SeedStatus, SeedStore};
use wellfed_core::store::update::{Bank0, Candidate, State, UpdateStatus, UpdateStore};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};

/// `bytes` as lowercase hex, 32 bytes a line.
fn lines(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(32)
        .map(|line| line.iter().map(|b| format!("{b:02x}")).collect())
        .collect()
}

/// Unit `slot` must hold exactly `golden` followed by erased bytes; a
/// mismatch prints the record found in the form this file pins it.
fn pinned(name: &str, f: &FakeJournalFlash, slot: usize, len: usize, golden: &[&str]) {
    let found = lines(&f.bytes[slot][..len]);
    if found != golden {
        let shown: Vec<String> = found
            .iter()
            .map(|l| format!("            \"{l}\","))
            .collect();
        panic!("{name} changed; found:\n{}", shown.join("\n"));
    }
    assert!(
        f.bytes[slot][len..].iter().all(|&b| b == u8::MAX),
        "{name}: bytes after the record"
    );
}

const JOURNAL: JournalConfig = JournalConfig {
    binding: 0x91a5_7320_ae11_c008,
    mode: BowlMode::Double,
    max_portions: 3,
};

fn request(j: &FeedJournal<FlashIo>, id: u64, portions: u16) -> FeedRequest {
    FeedRequest {
        id,
        portions,
        mode: j.config.mode,
        direction: j.view.recovery.direction,
        credit_next_event: j.view.recovery.credit_next_event,
        phase_known: j.view.recovery.phase_known,
    }
}

/// A finished double-bowl feed of `requested` portions.
fn finish(j: &mut FeedJournal<FlashIo>, outcome: OwnerOutcome) -> bool {
    let mut phase = j.view.recovery;
    phase.pending_request = false;
    let mut result = j.view.result;
    match outcome {
        OwnerOutcome::Done => {
            result.completed = result.requested;
            result.reason = FeedReason::Done;
            result.motion_possible = true;
            if result.requested & 1 == 0 {
                phase.direction = phase.direction.toggled();
            }
            phase.credit_next_event = true;
        }
        OwnerOutcome::Cancelled => {
            result.reason = FeedReason::Cancelled;
            result.motion_possible = false;
        }
        _ => {
            result.reason = FeedReason::TotalTimeout;
            result.motion_possible = false;
        }
    }
    result.outcome = outcome;
    j.finish(&result, &phase)
}

#[test]
fn journal_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut j = FeedJournal::default();
    // SAFETY: the fake flash outlives the journal.
    let created = unsafe { j.create(f.io(), JOURNAL, Direction::D1, false, true) };
    assert_eq!(created, JournalError::Ok);
    pinned("genesis", &f, 0, 128, GENESIS);

    assert!(j.reserve(&request(&j, 0x0102_0304_0506_0708, 2)));
    pinned("reservation", &f, 1, 128, RESERVATION);
    assert!(finish(&mut j, OwnerOutcome::Done));
    pinned("done", &f, 0, 128, DONE);

    assert!(j.reserve(&request(&j, 0x0102_0304_0506_0709, 3)));
    assert!(finish(&mut j, OwnerOutcome::Cancelled));
    pinned("cancelled", &f, 0, 128, CANCELLED);

    assert!(j.reserve(&request(&j, 0x0102_0304_0506_070a, 1)));
    assert!(finish(&mut j, OwnerOutcome::Expired));
    pinned("expired", &f, 0, 128, EXPIRED);

    // A reservation the next boot resolves as uncertain.
    assert!(j.reserve(&request(&j, 0x0102_0304_0506_070b, 2)));
    let mut j = FeedJournal::default();
    // SAFETY: as above.
    assert_eq!(unsafe { j.open(f.io(), JOURNAL) }, JournalError::Ok);
    let resolved = j.resolve().expect("the reservation resolves");
    assert_eq!(
        resolved,
        OwnerResult {
            id: 0x0102_0304_0506_070b,
            requested: 2,
            completed: 0,
            outcome: OwnerOutcome::Uncertain,
            reason: FeedReason::None,
            motion_possible: true,
        }
    );
    pinned("uncertain", &f, 0, 128, UNCERTAIN);

    // A single bowl's genesis carries its mode and the known phase.
    let mut f = FakeJournalFlash::with_unit(512);
    let mut j = FeedJournal::default();
    let single = JournalConfig {
        mode: BowlMode::Single,
        ..JOURNAL
    };
    // SAFETY: as above.
    let created = unsafe { j.create(f.io(), single, Direction::D0, true, true) };
    assert_eq!(created, JournalError::Ok);
    pinned("single genesis", &f, 0, 128, SINGLE_GENESIS);
}

#[test]
fn seed_records() {
    let mut f = FakeJournalFlash::with_unit(512);
    let mut s = SeedStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };
    let seed: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(7) ^ 0xa5);
    assert!(s.save(&seed));
    pinned("first seed", &f, 0, 128, FIRST_SEED);
    assert!(s.save(&[0xff; 32]));
    pinned("second seed", &f, 1, 128, SECOND_SEED);
}

fn candidate(version: u32, build: u8, kind: ImageKind) -> Candidate {
    Candidate {
        digest: core::array::from_fn(|i| build ^ (i as u8)),
        version,
        build_id: core::array::from_fn(|i| build.wrapping_add(i as u8)),
        kind,
    }
}

#[test]
fn update_records() {
    let mut f = FakeJournalFlash::with_unit(1024);
    let mut s = UpdateStore::default();
    // SAFETY: the fake flash outlives the store.
    unsafe { s.open(f.io()) };

    s.set_bank0(Bank0 {
        build_id: [0x31; 32],
        len: 942_080,
        sha256: core::array::from_fn(|i| 0x80 | (i as u8)),
        key: None,
    })
    .unwrap();
    pinned("bank 0 without a key", &f, 0, 256, BANK_0_WITHOUT_A_KEY);

    s.set_pending(candidate(0x0001_0203, 0x40, ImageKind::Replacement))
        .unwrap();
    pinned("pending", &f, 1, 256, PENDING);

    s.set_activated(candidate(0x0001_0203, 0x40, ImageKind::Replacement))
        .unwrap();
    pinned("activated", &f, 0, 256, ACTIVATED);

    s.confirm(&candidate(0x0001_0203, 0x40, ImageKind::Replacement).build_id)
        .unwrap();
    pinned("confirmed", &f, 1, 256, CONFIRMED);

    s.add_key(&[0x5a; 32]).unwrap();
    s.revoke(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    s.revoke(&[9; 8]).unwrap();
    s.set_bank0(Bank0 {
        build_id: [0x32; 32],
        len: 0x0012_3456,
        sha256: [0x77; 32],
        key: Some([0xc3; 8]),
    })
    .unwrap();
    s.set_pending(candidate(0x0002_0000, 0x50, ImageKind::Stock))
        .unwrap();
    pinned(
        "keys, bank 0 with a key and a stock candidate",
        &f,
        0,
        256,
        KEYS_AND_A_STOCK_CANDIDATE,
    );
}

/// Erased units of `unit` bytes holding each `(slot, record)` given.
fn flash(unit: usize, records: &[(usize, &[&str])]) -> FakeJournalFlash {
    let mut f = FakeJournalFlash::with_unit(unit);
    for &(slot, lines) in records {
        let hex: String = lines.concat();
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let pair = core::str::from_utf8(pair).unwrap();
            f.bytes[slot][i] = u8::from_str_radix(pair, 16).unwrap();
        }
    }
    f
}

fn open_journal(f: &mut FakeJournalFlash, config: JournalConfig) -> FeedJournal<FlashIo> {
    let mut j = FeedJournal::default();
    // SAFETY: the fake flash outlives the journal.
    assert_eq!(unsafe { j.open(f.io(), config) }, JournalError::Ok);
    j
}

#[test]
fn journal_reads_the_pinned_records() {
    let mut f = flash(512, &[(0, GENESIS)]);
    let v = open_journal(&mut f, JOURNAL).read().unwrap();
    assert_eq!((v.sequence, v.has_result), (1, false));
    assert_eq!(v.recovery.direction, Direction::D1);
    assert!(!v.recovery.credit_next_event && v.recovery.phase_known);
    assert!(!v.recovery.pending_request && v.recovery.high_water_id == 0);

    let mut f = flash(512, &[(0, GENESIS), (1, RESERVATION)]);
    let j = open_journal(&mut f, JOURNAL);
    let v = j.view;
    assert_eq!(
        (v.sequence, v.recovery.high_water_id),
        (2, 0x0102_0304_0506_0708)
    );
    assert!(v.recovery.pending_request && v.result.requested == 2);

    let mut f = flash(512, &[(1, RESERVATION), (0, DONE)]);
    let v = open_journal(&mut f, JOURNAL).view;
    assert_eq!(v.sequence, 3);
    assert_eq!(
        v.result,
        OwnerResult {
            id: 0x0102_0304_0506_0708,
            requested: 2,
            completed: 2,
            outcome: OwnerOutcome::Done,
            reason: FeedReason::Done,
            motion_possible: true,
        }
    );
    assert_eq!(
        v.recovery.direction,
        Direction::D0,
        "two portions turn the bowl"
    );
    assert!(v.recovery.credit_next_event && !v.recovery.pending_request);

    for (record, sequence, id, outcome, reason) in [
        (
            CANCELLED,
            5,
            0x0102_0304_0506_0709,
            OwnerOutcome::Cancelled,
            FeedReason::Cancelled,
        ),
        (
            EXPIRED,
            7,
            0x0102_0304_0506_070a,
            OwnerOutcome::Expired,
            FeedReason::TotalTimeout,
        ),
        (
            UNCERTAIN,
            9,
            0x0102_0304_0506_070b,
            OwnerOutcome::Uncertain,
            FeedReason::None,
        ),
    ] {
        let mut f = flash(512, &[(0, record)]);
        let v = open_journal(&mut f, JOURNAL).view;
        assert_eq!(
            (v.sequence, v.result.id, v.recovery.high_water_id),
            (sequence, id, id)
        );
        assert_eq!((v.result.outcome, v.result.reason), (outcome, reason));
        assert_eq!(v.result.motion_possible, outcome == OwnerOutcome::Uncertain);
        assert_eq!(v.result.completed, 0);
    }
    // An uncertain double-bowl result leaves the phase unknown.
    let mut f = flash(512, &[(0, UNCERTAIN)]);
    assert!(!open_journal(&mut f, JOURNAL).view.recovery.phase_known);

    let single = JournalConfig {
        mode: BowlMode::Single,
        ..JOURNAL
    };
    let mut f = flash(512, &[(0, SINGLE_GENESIS)]);
    let v = open_journal(&mut f, single).view;
    assert_eq!((v.sequence, v.recovery.direction), (1, Direction::D0));
    assert!(v.recovery.credit_next_event && v.recovery.phase_known);
}

#[test]
fn seed_reads_the_pinned_records() {
    let mut f = flash(512, &[(0, FIRST_SEED)]);
    let mut s = SeedStore::default();
    // SAFETY: the fake flash outlives the store.
    assert_eq!(unsafe { s.open(f.io()) }, SeedStatus::Loaded);
    let seed: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(7) ^ 0xa5);
    assert_eq!((s.generation, s.seed()), (1, Some(&seed)));

    let mut f = flash(512, &[(0, FIRST_SEED), (1, SECOND_SEED)]);
    // SAFETY: as above.
    unsafe { s.open(f.io()) };
    assert_eq!((s.generation, s.slot, s.seed()), (2, 1, Some(&[0xff; 32])));
}

fn open_update(record: &[&str]) -> UpdateStore<FlashIo> {
    // The store outlives this function's flash, so the flash is leaked.
    let f = Box::leak(Box::new(flash(1024, &[(0, record)])));
    let mut s = UpdateStore::default();
    // SAFETY: the leaked flash lives for the rest of the test.
    assert_eq!(unsafe { s.open(f.io()) }, UpdateStatus::Loaded);
    s
}

#[test]
fn update_store_reads_the_pinned_records() {
    let first_bank0 = Bank0 {
        build_id: [0x31; 32],
        len: 942_080,
        sha256: core::array::from_fn(|i| 0x80 | (i as u8)),
        key: None,
    };
    let replacement = candidate(0x0001_0203, 0x40, ImageKind::Replacement);

    let s = open_update(BANK_0_WITHOUT_A_KEY);
    let r = *s.record();
    assert_eq!((s.generation, r.min_version, r.state), (1, 0, State::None));
    assert_eq!(
        (r.bank0, r.added, r.revoked().len()),
        (Some(first_bank0), None, 0)
    );

    let s = open_update(PENDING);
    assert_eq!(
        (s.generation, s.record().state),
        (2, State::Pending(replacement))
    );
    let s = open_update(ACTIVATED);
    assert_eq!(
        (s.generation, s.record().state),
        (3, State::Activated(replacement))
    );

    let s = open_update(CONFIRMED);
    let r = *s.record();
    assert_eq!(
        (s.generation, r.min_version, r.state),
        (4, 0x0001_0203, State::None)
    );
    assert_eq!(r.bank0, Some(first_bank0));

    let s = open_update(KEYS_AND_A_STOCK_CANDIDATE);
    let r = *s.record();
    assert_eq!(s.generation, 9);
    assert_eq!(r.min_version, 0x0001_0203);
    assert_eq!(
        r.state,
        State::Pending(candidate(0x0002_0000, 0x50, ImageKind::Stock))
    );
    assert_eq!(r.added, Some([0x5a; 32]));
    assert_eq!(r.revoked(), &[[1, 2, 3, 4, 5, 6, 7, 8], [9; 8]]);
    assert_eq!(
        r.bank0,
        Some(Bank0 {
            build_id: [0x32; 32],
            len: 0x0012_3456,
            sha256: [0x77; 32],
            key: Some([0xc3; 8]),
        })
    );
}

// The pinned records, as deployed feeders hold them.

/// Genesis.
const GENESIS: &[&str] = &[
    "504c464a4e4c30310100800000010102010000000000000008c011ae2073a591",
    "0000000000000000000000000000030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000000b45dd5cb434d4954",
];

/// Reservation.
const RESERVATION: &[&str] = &[
    "504c464a4e4c30310100800001010102020000000000000008c011ae2073a591",
    "0807060504030201020000000000030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000000000000000000068efeb55434d4954",
];

/// Done.
const DONE: &[&str] = &[
    "504c464a4e4c30310100800002010007030000000000000008c011ae2073a591",
    "0807060504030201020002000101030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000000000000000000098dbedf8434d4954",
];

/// Cancelled.
const CANCELLED: &[&str] = &[
    "504c464a4e4c30310100800002010003050000000000000008c011ae2073a591",
    "0907060504030201030000000202030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000000000000000000082b4a4e2434d4954",
];

/// Expired.
const EXPIRED: &[&str] = &[
    "504c464a4e4c30310100800002010003070000000000000008c011ae2073a591",
    "0a07060504030201010000000305030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000007730902d434d4954",
];

/// Uncertain.
const UNCERTAIN: &[&str] = &[
    "504c464a4e4c30310100800002010005090000000000000008c011ae2073a591",
    "0b07060504030201020000000500030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000000e7e0c692434d4954",
];

/// Single genesis.
const SINGLE_GENESIS: &[&str] = &[
    "504c464a4e4c30310100800000000003010000000000000008c011ae2073a591",
    "0000000000000000000000000000030000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000002434cf23434d4954",
];

/// First seed.
const FIRST_SEED: &[&str] = &[
    "504c465345454431010080000100000000000000a5a2abb0b9868f949d9ae3e8",
    "f1fec7ccd5d2db2029363f040d0a1318616e777c000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000000536f144e434d4954",
];

/// Second seed.
const SECOND_SEED: &[&str] = &[
    "504c465345454431010080000200000000000000ffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffff000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000005625eaf5434d4954",
];

/// Bank 0 without a key.
const BANK_0_WITHOUT_A_KEY: &[&str] = &[
    "504c465550443031010000010100000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0100000000600e00313131313131313131313131313131313131313131313131",
    "3131313131313131808182838485868788898a8b8c8d8e8f9091929394959697",
    "98999a9b9c9d9e9f00000000000000000000000000000000a716c65a434d4954",
];

/// Pending.
const PENDING: &[&str] = &[
    "504c465550443031010000010200000000000000000000000101000003020100",
    "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
    "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0100000000600e00313131313131313131313131313131313131313131313131",
    "3131313131313131808182838485868788898a8b8c8d8e8f9091929394959697",
    "98999a9b9c9d9e9f000000000000000000000000000000005f209156434d4954",
];

/// Activated.
const ACTIVATED: &[&str] = &[
    "504c465550443031010000010300000000000000000000000201000003020100",
    "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
    "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0100000000600e00313131313131313131313131313131313131313131313131",
    "3131313131313131808182838485868788898a8b8c8d8e8f9091929394959697",
    "98999a9b9c9d9e9f00000000000000000000000000000000a37b3c4c434d4954",
];

/// Confirmed.
const CONFIRMED: &[&str] = &[
    "504c465550443031010000010400000000000000030201000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0100000000600e00313131313131313131313131313131313131313131313131",
    "3131313131313131808182838485868788898a8b8c8d8e8f9091929394959697",
    "98999a9b9c9d9e9f00000000000000000000000000000000025bc733434d4954",
];

/// Keys, bank 0 with a key and a stock candidate.
const KEYS_AND_A_STOCK_CANDIDATE: &[&str] = &[
    "504c465550443031010000010900000000000000030201000102010200000200",
    "505152535455565758595a5b5c5d5e5f404142434445464748494a4b4c4d4e4f",
    "505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f",
    "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
    "0102030405060708090909090909090900000000000000000000000000000000",
    "0101000056341200323232323232323232323232323232323232323232323232",
    "3232323232323232777777777777777777777777777777777777777777777777",
    "7777777777777777c3c3c3c3c3c3c3c30000000000000000807390db434d4954",
];
