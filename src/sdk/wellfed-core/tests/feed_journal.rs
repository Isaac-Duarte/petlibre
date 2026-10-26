//! Recovery tests for the two-slot feed journal: lifecycle, phase recovery,
//! bit-flip corruption, binding mismatch, torn writes cut at every byte,
//! injected I/O failures, the pinned 128-byte v1 wire format, exhaustion,
//! the wear limit, and the uncertain resolution of an interrupted
//! reservation.
//!
//! The invariant under test: after any cut or failure, reopening yields the
//! committed state, an unresolved pending intent, or `CORRUPT`, never an idle
//! journal that has forgotten a reserved request ID. A cut result or
//! resolution always yields the pending intent or the committed result, and
//! a cut reservation the committed state or a pending intent: no cut ever
//! faults the journal.

use wellfed_core::feed::controller::{FeedReason, FeedRequest};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::storage::{
    OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage, RotorPhase,
};
use wellfed_core::store::feed_journal::{FeedJournal, JournalConfig, JournalError};
use wellfed_core::store::slot::{RECORD_BYTES, SlotIo};
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo, UNIT};

/// Unit sizes for torn-write/restart coverage: the virtual feeder's 256, a
/// 4 KiB sector, and units whose blank tail ends in a chunk shorter than the
/// journal's 64-byte scan buffer (4116: 62 full chunks + 20; 148: 20 only).
const UNITS: [usize; 4] = [UNIT, 4096, 4116, 148];

const CONFIG: JournalConfig = JournalConfig {
    binding: 0x91a5_7320_ae11_c008,
    mode: BowlMode::Single,
    max_portions: 3,
};

fn request(j: &FeedJournal<FlashIo>, id: u64) -> FeedRequest {
    FeedRequest {
        id,
        portions: 2,
        mode: j.config.mode,
        direction: j.view.recovery.direction,
        credit_next_event: j.view.recovery.credit_next_event,
        phase_known: j.view.recovery.phase_known,
        homing: false,
    }
}

fn reserve(j: &mut FeedJournal<FlashIo>, id: u64) -> bool {
    let r = request(j, id);
    j.reserve(&r)
}

fn finish(j: &mut FeedJournal<FlashIo>, done: bool, motion: bool) -> bool {
    let mut phase = j.view.recovery;
    phase.pending_request = false;

    let mut result = j.view.result;
    result.completed = if done { result.requested } else { 0 };
    result.outcome = if done {
        OwnerOutcome::Done
    } else {
        OwnerOutcome::Cancelled
    };
    result.reason = if done {
        FeedReason::Done
    } else {
        FeedReason::Cancelled
    };
    result.motion_possible = motion;

    if j.config.mode == BowlMode::Double {
        if done {
            if result.requested & 1 == 0 {
                phase.direction = phase.direction.toggled();
            }
            phase.credit_next_event = true;
        } else if motion {
            phase.phase_known = false;
        }
    }

    j.finish(&result, &phase)
}

fn create(f: &mut FakeJournalFlash, dual: bool) -> FeedJournal<FlashIo> {
    create_unit(f, dual, UNIT)
}

fn create_unit(f: &mut FakeJournalFlash, dual: bool, unit: usize) -> FeedJournal<FlashIo> {
    *f = FakeJournalFlash::with_unit(unit);

    let config = JournalConfig {
        mode: if dual {
            BowlMode::Double
        } else {
            BowlMode::Single
        },
        ..CONFIG
    };

    let mut j = FeedJournal::default();
    // SAFETY: each test keeps `f` in place while the journal uses it.
    assert_eq!(
        // SAFETY: the fixture's callbacks are sound for its context, which outlives the call.
        unsafe { j.create(f.io(), config, Direction::D0, true, true) },
        JournalError::Ok
    );

    j
}

fn reopen(
    f: &mut FakeJournalFlash,
    j: &mut FeedJournal<FlashIo>,
    config: &JournalConfig,
) -> JournalError {
    f.budget = -1;
    f.fail_call = 0;
    // SAFETY: as for `create`.
    unsafe { j.open(f.io(), *config) }
}

/// Point a copied journal at a copied flash.
fn bind(j: &mut FeedJournal<FlashIo>, f: &mut FakeJournalFlash) {
    // SAFETY: the test's flash stays in place while the journal uses it.
    j.rebind(unsafe { f.io() });
}

/// A unit size no record layout fits.
struct OddUnit(FlashIo);

impl SlotIo for OddUnit {
    fn unit_bytes(&self) -> u32 {
        127
    }
    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.0.read(slot, offset, bytes)
    }
    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        self.0.program(slot, offset, bytes)
    }
    fn erase(&self, slot: u32) -> bool {
        self.0.erase(slot)
    }
}

#[test]
fn lifecycle() {
    let mut f = FakeJournalFlash::default();
    let mut j = FeedJournal::default();
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Empty);
    assert_eq!(f.mutations, 0);

    let mut j = create(&mut f, false);
    assert!(j.view.sequence == 1 && j.view.recovery.high_water_id == 0);

    assert!(reserve(&mut j, 0x1_0000_0001));
    assert!(j.view.sequence == 2 && j.view.recovery.pending_request);
    assert!(!reserve(&mut j, 0x1_0000_0002));
    assert!(finish(&mut j, true, true));

    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    let v = j.read().unwrap();
    assert!(v.has_result && !v.recovery.pending_request);
    assert!(v.result.id == 0x1_0000_0001 && v.result.completed == 2);
    assert!(!reserve(&mut j, v.result.id) && !reserve(&mut j, 1));
    assert!(reserve(&mut j, 0x1_0000_0010) && finish(&mut j, false, false));
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    assert!(j.view.sequence == 5 && j.view.result.outcome == OwnerOutcome::Cancelled);

    let writes = f.mutations;
    assert_eq!(
        // SAFETY: the fixture's callbacks are sound for its context, which outlives the call.
        unsafe { j.create(f.io(), CONFIG, Direction::D0, true, true) },
        JournalError::State
    );
    assert_eq!(f.mutations, writes);
}

#[test]
fn phase_and_recovery() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, true);
    let c = j.config;
    assert!(reserve(&mut j, 10));

    let mut rebooted = FeedJournal::default();
    assert_eq!(reopen(&mut f, &mut rebooted, &c), JournalError::Ok);
    let mut phase = OwnerRecovery::default();
    assert!(OwnerStorage::recover(&mut rebooted, &mut phase));
    assert!(phase.pending_request && !phase.phase_known);
    assert_eq!(phase.high_water_id, 10);
    assert!(!reserve(&mut rebooted, 11) && !finish(&mut rebooted, true, true));

    bind(&mut j, &mut f);
    assert!(finish(&mut j, true, true));
    assert!(j.view.recovery.direction == Direction::D1 && j.view.recovery.credit_next_event);
    assert!(reserve(&mut j, 11) && finish(&mut j, false, true));
    assert_eq!(reopen(&mut f, &mut j, &c), JournalError::Ok);
    assert!(!j.view.recovery.phase_known && !reserve(&mut j, 12));

    let mut j = create(&mut f, true);
    assert!(reserve(&mut j, 1) && finish(&mut j, false, false));
    assert!(j.view.recovery.phase_known && reserve(&mut j, 2));
}

#[test]
fn invalid_arguments() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, false);
    let mut r = request(&j, 1);
    let writes = f.mutations;

    r.portions = 0;
    assert!(!j.reserve(&r));
    r.portions = 4;
    assert!(!j.reserve(&r));
    r.portions = 2;
    r.mode = BowlMode::Double;
    assert!(!j.reserve(&r));
    r.mode = BowlMode::Single;
    r.direction = Direction::D1;
    assert!(!j.reserve(&r));
    assert!(f.mutations == writes && reserve(&mut j, 7));

    let mut phase = j.view.recovery;
    phase.pending_request = false;
    let mut result = OwnerResult {
        id: 7,
        requested: 2,
        completed: 2,
        outcome: OwnerOutcome::Done,
        reason: FeedReason::Done,
        motion_possible: true,
    };

    result.id = 6;
    assert!(!j.finish(&result, &phase));
    result.id = 7;
    result.completed = 1;
    assert!(!j.finish(&result, &phase));
    result.completed = 2;
    result.motion_possible = false;
    assert!(!j.finish(&result, &phase));
    result.motion_possible = true;
    phase.phase_known = false;
    assert!(!j.finish(&result, &phase));
    phase.phase_known = true;
    assert!(j.finish(&result, &phase));

    // SAFETY: the test's flash stays in place while the journal uses it.
    let io = OddUnit(unsafe { f.io() });
    let mut odd = FeedJournal::default();
    assert_eq!(odd.open(io, CONFIG), JournalError::Argument);
    assert!(odd.read().is_none());
    // SAFETY: as above.
    let io = unsafe { f.io() };
    let unbound = JournalConfig {
        binding: 0,
        ..CONFIG
    };
    assert_eq!(j.open(io, unbound), JournalError::Argument);
    assert!(j.read().is_none());
}

#[test]
fn corruption_and_binding() {
    let mut original = FakeJournalFlash::default();
    let mut j = create(&mut original, false);
    assert!(reserve(&mut j, 41) && finish(&mut j, true, true));

    for slot in 0..2 {
        for byte in 0..RECORD_BYTES {
            for bit in 0..8 {
                let mut f = original.clone();
                f.bytes[slot][byte] ^= 1 << bit;
                let e = reopen(&mut f, &mut j, &CONFIG);
                if e == JournalError::Ok {
                    // Only safe fallback is an unresolved intent, never idle:
                    // the result's own reservation (a flip in the result), or
                    // the next one (a flip in the older reservation, beside
                    // the result at rest).
                    let high = j.view.recovery.high_water_id;
                    assert!(
                        j.view.recovery.pending_request
                            && (high == 41 || (slot == 1 && high == 42))
                    );
                    assert!(!reserve(&mut j, 43) && !finish(&mut j, true, true));
                } else {
                    assert_eq!(e, JournalError::Corrupt);
                }
            }
        }
    }

    let mut f = original.clone();
    f.bytes[0][200] = 0;
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);

    // Every blank-tail chunk is checked, including a short final chunk.
    for unit in UNITS {
        let mut original = FakeJournalFlash::default();
        let mut j = create_unit(&mut original, false, unit);
        assert!(reserve(&mut j, 41) && finish(&mut j, true, true));
        let bytes = [RECORD_BYTES, RECORD_BYTES + 64, unit - 20, unit - 1];
        for byte in bytes.into_iter().filter(|&byte| byte < unit) {
            for slot in 0..2 {
                let mut f = original.clone();
                f.bytes[slot][byte] = 0xfe;
                assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);
            }
        }
        let mut f = original.clone();
        assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
        assert_eq!(j.view.recovery.high_water_id, 41);
    }

    let c = JournalConfig {
        binding: CONFIG.binding + 1,
        ..CONFIG
    };
    assert_eq!(reopen(&mut original, &mut j, &c), JournalError::Corrupt);

    let c = JournalConfig {
        mode: BowlMode::Double,
        ..CONFIG
    };
    assert_eq!(reopen(&mut original, &mut j, &c), JournalError::Corrupt);

    let c = JournalConfig {
        max_portions: CONFIG.max_portions + 1,
        ..CONFIG
    };
    assert_eq!(reopen(&mut original, &mut j, &c), JournalError::Corrupt);
}

fn byte_cuts(completing: bool, reused: bool, unit: usize) {
    let mut original = FakeJournalFlash::default();
    let mut original_j = create_unit(&mut original, false, unit);
    if reused {
        assert!(reserve(&mut original_j, 9) && finish(&mut original_j, true, true));
    }
    if completing {
        assert!(reserve(&mut original_j, 42));
    }

    // Erase is `unit` mutations, then the body and the commit marker. Every
    // cut near the erase boundaries and through the record; large units sample
    // the middle of the erase.
    let end = (unit + RECORD_BYTES + 1) as i64;
    let edge = 2 * RECORD_BYTES as i64;
    let cuts = (0..=end).filter(|&cut| cut <= edge || cut >= unit as i64 - edge || cut % 61 == 0);

    for cut in cuts {
        let mut f = original.clone();
        let mut j = original_j;
        f.budget = cut;
        bind(&mut j, &mut f);

        let succeeded = if completing {
            finish(&mut j, true, true)
        } else {
            reserve(&mut j, 42)
        };

        let writes = f.mutations;
        if !succeeded {
            assert!(!j.opened && !reserve(&mut j, 43) && !finish(&mut j, true, true));
            assert_eq!(f.mutations, writes);
        }

        // A cut result leaves the reservation pending, to be resolved at the
        // next boot; a cut reservation leaves the record at rest (an erase
        // that found its unit erased) or stands as the next reservation.
        assert_eq!(
            reopen(&mut f, &mut j, &CONFIG),
            JournalError::Ok,
            "cut {cut}"
        );
        let rest = if reused { 9 } else { 0 };
        let high = j.view.recovery.high_water_id;
        let stand_in = !completing && j.view.recovery.pending_request && high != 42;
        assert!(high == 42 || (!completing && (high == rest || (stand_in && high == rest + 1))));
        if completing {
            assert!(
                j.view.recovery.pending_request
                    || (j.view.has_result && j.view.result.outcome == OwnerOutcome::Done)
            );
        }
        if succeeded {
            assert!(high == 42 && j.view.recovery.pending_request != completing);
        }
        if stand_in {
            assert_eq!(high, rest + 1, "the next ID, which the loop reserves");
            assert!(!reserve(&mut j, 43) && !finish(&mut j, true, true));
            let resolved = j.resolve().expect("the stand-in resolves");
            assert_eq!(resolved.outcome, OwnerOutcome::Uncertain);
            assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
            assert!(j.view.has_result && !j.view.recovery.pending_request);
            assert_eq!(j.view.recovery.high_water_id, rest + 1);
            assert!(reserve(&mut j, rest + 2));
        }
    }
}

#[test]
fn byte_cut_recovery() {
    for unit in UNITS {
        byte_cuts(false, false, unit);
        byte_cuts(false, true, unit);
        byte_cuts(true, false, unit);
        byte_cuts(true, true, unit);
    }
}

#[test]
fn create_cuts() {
    for (unit, cut) in UNITS
        .into_iter()
        .flat_map(|unit| (0..=(RECORD_BYTES + 1) as i64).map(move |cut| (unit, cut)))
    {
        let mut f = FakeJournalFlash {
            budget: cut,
            ..FakeJournalFlash::with_unit(unit)
        };
        let mut j = FeedJournal::default();
        // SAFETY: the fixture's callbacks are sound for its context, which outlives the call.
        let before = unsafe { j.create(f.io(), CONFIG, Direction::D0, true, true) };
        let after = reopen(&mut f, &mut j, &CONFIG);

        if after == JournalError::Ok {
            assert!(j.view.sequence == 1 && j.view.recovery.high_water_id == 0);
        } else {
            assert!(
                before != JournalError::Ok
                    && (after == JournalError::Empty || after == JournalError::Corrupt)
            );
        }
    }
}

#[test]
fn io_failures() {
    for unit in UNITS {
        io_failures_at(unit);
    }
}

fn io_failures_at(unit: usize) {
    let mut original = FakeJournalFlash::default();
    let original_j = create_unit(&mut original, false, unit);
    for completing in [false, true] {
        let mut base = original.clone();
        let mut source = original_j;
        bind(&mut source, &mut base);
        if completing {
            assert!(reserve(&mut source, 42));
        }
        // Every call of the write (erase, blank scan in 64-byte chunks, body,
        // verify, commit, verify), so larger units cover all their scan reads.
        let calls = {
            let mut f = base.clone();
            let mut j = source;
            bind(&mut j, &mut f);
            let start = f.calls;
            assert!(if completing {
                finish(&mut j, true, true)
            } else {
                reserve(&mut j, 42)
            });
            f.calls - start
        };

        assert!(calls >= 6);
        for failure in 1..=calls {
            let mut f = base.clone();
            f.fail_call = f.calls + failure;
            let mut j = source;
            bind(&mut j, &mut f);

            let ok = if completing {
                finish(&mut j, true, true)
            } else {
                reserve(&mut j, 42)
            };

            let e = reopen(&mut f, &mut j, &CONFIG);
            if ok {
                assert!(e == JournalError::Ok && j.view.recovery.high_water_id == 42);
            }
            if completing && e == JournalError::Ok {
                assert_eq!(j.view.recovery.high_water_id, 42);
            }
        }
    }

    let mut f = original.clone();
    f.silent_program = true;
    let mut j = original_j;
    bind(&mut j, &mut f);
    assert!(!reserve(&mut j, 42) && !j.opened);
    f.silent_program = false;
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    assert!(reserve(&mut j, 42));

    f.silent_erase = true; // Inactive slot still holds the genesis record.
    bind(&mut j, &mut f);
    assert!(!finish(&mut j, true, true) && !j.opened);
}

/// Independent format mutations use a deliberately different CRC formulation.
fn repair_crc(p: &mut [u8]) {
    let mut remainder = u32::MAX;
    for &input in &p[..120] {
        let mut byte = input;
        for _ in 0..8 {
            let carry = (remainder ^ u32::from(byte)) & 1 != 0;
            remainder >>= 1;
            if carry {
                remainder ^= 0xedb8_8320;
            }
            byte >>= 1;
        }
    }

    p[120..124].copy_from_slice(&(!remainder).to_le_bytes());
}

#[test]
fn wire_format_and_invalid_transitions() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, false);
    // Python struct.pack/zlib.crc32 reference, not a native struct dump.
    let mut expected = [0u8; RECORD_BYTES];
    expected[..16].copy_from_slice(&[
        0x50, 0x4c, 0x46, 0x4a, 0x4e, 0x4c, 0x30, 0x31, 1, 0, 0x80, 0, 0, 0, 0, 3,
    ]);
    expected[16] = 1;
    expected[24..32].copy_from_slice(&[8, 0xc0, 0x11, 0xae, 0x20, 0x73, 0xa5, 0x91]);
    expected[46] = 3;
    expected[120..].copy_from_slice(&[0x24, 0x34, 0xcf, 0x23, 0x43, 0x4d, 0x49, 0x54]);
    assert_eq!(f.bytes[0][..RECORD_BYTES], expected);

    assert!(reserve(&mut j, 42) && finish(&mut j, true, true));
    let original = f.clone();
    for field in [
        8, 10, 12, 13, 14, 15, 16, 24, 32, 40, 42, 44, 45, 46, 48, 119,
    ] {
        let mut f = original.clone();
        f.bytes[0][field] ^= 0x80;
        repair_crc(&mut f.bytes[0]);
        assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);
    }

    let mut f = original.clone();
    f.bytes[0][16] = 5; // Valid odd result generation, but skips generation 3.
    repair_crc(&mut f.bytes[0]);
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);

    let mut f = original.clone();
    f.bytes[1] = f.bytes[0].clone();
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);
}

#[test]
fn id_and_sequence_exhaustion() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, false);
    assert!(reserve(&mut j, u64::MAX) && finish(&mut j, true, true));
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    assert!(j.view.recovery.high_water_id == u64::MAX && !reserve(&mut j, 1));

    // Synthesize the last adjacent generations, preserving valid checksums.
    for slot in 0..2 {
        f.bytes[slot][16..24].fill(u8::MAX);
        if slot == 1 {
            f.bytes[slot][16] = 0xfe;
        }
        repair_crc(&mut f.bytes[slot]);
    }
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    assert_eq!(j.view.sequence, u64::MAX);

    // Restore an ordinary high-water to distinguish sequence from ID limit.
    for slot in 0..2 {
        f.bytes[slot][32..40].fill(0);
        f.bytes[slot][32] = 1;
        repair_crc(&mut f.bytes[slot]);
    }
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    let writes = f.mutations;
    assert!(!reserve(&mut j, 2) && j.error == JournalError::Exhausted && f.mutations == writes);
}

#[test]
fn wear_limit_refuses_new_reservations_but_never_strands_a_pending_one() {
    use wellfed_core::store::feed_journal::WEAR_LIMIT;

    let mut f = FakeJournalFlash::default();
    let mut j = FeedJournal::default();
    // SAFETY: the fake flash outlives the journal.
    assert_eq!(
        // SAFETY: the fixture's callbacks are sound for its context, which outlives the call.
        unsafe { j.create(f.io(), CONFIG, Direction::D0, false, true) },
        JournalError::Ok
    );

    assert!(reserve(&mut j, 1));
    j.view.sequence = WEAR_LIMIT;

    // The pending request can still finish.
    assert!(finish(&mut j, false, false));

    let writes = f.mutations;
    assert!(!reserve(&mut j, 2));
    assert_eq!(
        (j.error, j.opened, f.mutations),
        (JournalError::Worn, true, writes)
    );
    assert!(j.read().is_some());
}

/// The reservation `j` holds, reopened as the next boot finds it.
fn rebooted(f: &mut FakeJournalFlash, config: &JournalConfig) -> FeedJournal<FlashIo> {
    let mut j = FeedJournal::default();
    assert_eq!(reopen(f, &mut j, config), JournalError::Ok);
    assert!(j.view.recovery.pending_request && !j.reserved_here);
    j
}

fn assert_resolved(j: &FeedJournal<FlashIo>, sequence: u64) {
    let v = j.read().unwrap();
    assert!(v.has_result && !v.recovery.pending_request && v.sequence == sequence);
    assert_eq!(
        v.result,
        OwnerResult {
            id: 42,
            requested: 2,
            completed: 0,
            outcome: OwnerOutcome::Uncertain,
            reason: FeedReason::None,
            motion_possible: true,
        }
    );
    assert_eq!(v.recovery.high_water_id, 42);
}

#[test]
fn an_interrupted_reservation_resolves_once_as_uncertain() {
    for dual in [false, true] {
        let mut f = FakeJournalFlash::default();
        let mut j = create(&mut f, dual);
        let c = j.config;

        // Nothing pending, or a reservation this boot will finish: refused.
        let writes = f.mutations;
        assert!(j.resolve().is_none());
        assert!(reserve(&mut j, 42) && j.resolve().is_none());
        assert_eq!(f.mutations, writes + (UNIT + RECORD_BYTES) as u32);

        let mut j = rebooted(&mut f, &c);
        let mut result = j.view.result;
        result.outcome = OwnerOutcome::Uncertain;
        result.motion_possible = true;
        let mut phase = j.view.recovery;
        phase.pending_request = false;
        assert!(!j.finish(&result, &phase), "only a reboot resolves");

        let resolved = j.resolve().unwrap();
        assert_resolved(&j, 3);
        assert_eq!(resolved, j.view.result);
        assert_eq!(j.view.recovery.phase_known, !dual);
        assert!(j.resolve().is_none() && !reserve(&mut j, 42));

        // The resolution record: a result (type 2) with outcome 5, reason
        // none and motion possible, in the v1 layout.
        let record = &f.bytes[0][..RECORD_BYTES];
        assert_eq!(record[12], 2);
        assert_eq!(record[15] & 4, 4);
        assert_eq!(&record[40..46], &[2, 0, 0, 0, 5, 0]);

        assert_eq!(reopen(&mut f, &mut j, &c), JournalError::Ok);
        assert_resolved(&j, 3);
        if dual {
            assert!(!reserve(&mut j, 43), "a double bowl's phase is unknown");
        } else {
            assert!(reserve(&mut j, 43) && finish(&mut j, true, true));
        }
    }
}

fn resolve_cuts(reused: bool, unit: usize) {
    let mut original = FakeJournalFlash::default();
    let mut j = create_unit(&mut original, false, unit);
    if reused {
        assert!(reserve(&mut j, 9) && finish(&mut j, true, true));
    }
    assert!(reserve(&mut j, 42));
    let booted = rebooted(&mut original, &CONFIG);
    let pending = booted.view.sequence;

    let total = {
        let mut f = original.clone();
        let mut j = booted;
        bind(&mut j, &mut f);
        let start = f.mutations;
        assert!(j.resolve().is_some());
        i64::from(f.mutations - start)
    };

    // Every cut near the erase boundaries and through the record, as for
    // `byte_cuts`; large units sample the middle of the erase.
    let edge = 2 * RECORD_BYTES as i64;
    let unit = unit as i64;
    let cuts = (0..=total).filter(|&cut| cut <= edge || cut >= unit - edge || cut % 61 == 0);
    for cut in cuts {
        let mut f = original.clone();
        let mut j = booted;
        f.budget = cut;
        bind(&mut j, &mut f);
        let resolved = j.resolve();

        // The next boot finds the reservation or its resolution, never a
        // fault; a pending one resolves then, and only once.
        assert_eq!(
            reopen(&mut f, &mut j, &CONFIG),
            JournalError::Ok,
            "cut {cut}"
        );
        if j.view.recovery.pending_request {
            assert!(resolved.is_none() && j.view.sequence == pending);
            assert!(j.resolve().is_some());
            assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
        }
        assert_resolved(&j, pending + 1);
        assert!(j.resolve().is_none() && !reserve(&mut j, 42));
        assert!(reserve(&mut j, 43));
    }
}

#[test]
fn a_cut_resolution_leaves_the_reservation_or_its_result() {
    for unit in UNITS {
        resolve_cuts(false, unit);
        resolve_cuts(true, unit);
    }
}

#[test]
fn a_torn_record_beside_a_pending_one_resolves_but_a_foreign_one_faults() {
    let mut original = FakeJournalFlash::default();
    let mut j = create(&mut original, false);
    assert!(reserve(&mut j, 9) && finish(&mut j, true, true) && reserve(&mut j, 42));
    // Slot 0 holds the older result, slot 1 the pending reservation.

    // An erase cut over the older record raises bits: it fails its checksum.
    let mut f = original.clone();
    f.bytes[0][40] |= 0x80;
    let mut j = rebooted(&mut f, &CONFIG);
    assert!(j.resolve().is_some());
    assert_resolved(&j, 5);

    // A record that checks but does not decode is never a cut's.
    let mut f = original.clone();
    f.bytes[0][13] ^= 1;
    repair_crc(&mut f.bytes[0]);
    let mut j = FeedJournal::default();
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);

    // Nor are bytes past the record.
    let mut f = original.clone();
    f.bytes[0][200] = 0;
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Corrupt);

    // Beside a record at rest, a torn one is the next reservation, cut or
    // decayed: it resolves as uncertain under the next ID.
    let mut original = FakeJournalFlash::default();
    let mut j = create(&mut original, false);
    assert!(reserve(&mut j, 9) && finish(&mut j, true, true));
    let mut f = original.clone();
    f.bytes[1][40] |= 0x80;
    let mut j = rebooted(&mut f, &CONFIG);
    assert_eq!(j.view.recovery.high_water_id, 10);
    let resolved = j.resolve().unwrap();
    assert_eq!(
        resolved,
        OwnerResult {
            id: 10,
            requested: CONFIG.max_portions,
            completed: 0,
            outcome: OwnerOutcome::Uncertain,
            reason: FeedReason::None,
            motion_possible: true,
        }
    );
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
    assert!(j.view.has_result && j.view.recovery.high_water_id == 10);
    assert!(!reserve(&mut j, 10) && reserve(&mut j, 11));

    // A double bowl whose phase is unknown makes no reservation, so a cut
    // write beside it is not the journal's.
    let mut original = FakeJournalFlash::default();
    let mut j = create(&mut original, true);
    let dual = j.config;
    assert!(reserve(&mut j, 9) && finish(&mut j, false, true));
    assert!(!j.view.recovery.phase_known);
    let mut f = original.clone();
    f.bytes[1][40] |= 0x80;
    assert_eq!(reopen(&mut f, &mut j, &dual), JournalError::Corrupt);
}

/// A reservation cut beside a record at rest is resolved in two writes, the
/// reservation and then its result: a cut in either leaves the same
/// reservation, or its result, and it resolves once.
#[test]
fn a_cut_reservation_resolves_through_any_cut() {
    for unit in UNITS {
        for reused in [false, true] {
            let mut original = FakeJournalFlash::default();
            let mut j = create_unit(&mut original, false, unit);
            if reused {
                assert!(reserve(&mut j, 9) && finish(&mut j, true, true));
            }
            let rest = j.view.recovery.high_water_id;
            // Cut halfway through the reservation's record.
            original.budget = (unit + RECORD_BYTES / 2) as i64;
            assert!(!reserve(&mut j, rest + 1));
            let booted = rebooted(&mut original, &CONFIG);

            let total = {
                let mut f = original.clone();
                let mut j = booted;
                bind(&mut j, &mut f);
                let start = f.mutations;
                assert!(j.resolve().is_some());
                i64::from(f.mutations - start)
            };
            assert_eq!(total, 2 * (unit + RECORD_BYTES) as i64);

            let edge = 2 * RECORD_BYTES as i64;
            let unit = unit as i64;
            let near = |cut: i64, at: i64| (at - edge..=at + edge).contains(&cut);
            let cuts = (0..=total)
                .filter(|&cut| near(cut, 0) || near(cut, unit) || near(cut, total / 2 + unit));
            for cut in cuts.chain((0..=total).step_by(61)) {
                let mut f = original.clone();
                let mut j = booted;
                f.budget = cut;
                bind(&mut j, &mut f);
                let resolved = j.resolve();

                assert_eq!(
                    reopen(&mut f, &mut j, &CONFIG),
                    JournalError::Ok,
                    "cut {cut}"
                );
                if j.view.recovery.high_water_id == rest {
                    // Cut late in the erase, or before the first byte: the
                    // unit reads erased, as the reservation's own erase
                    // would have left it, and the feeder is where it was
                    // before the reservation began.
                    assert!(cut <= unit, "cut {cut}");
                    assert!(!j.view.recovery.pending_request && reserve(&mut j, rest + 1));
                    continue;
                }
                assert_eq!(j.view.recovery.high_water_id, rest + 1, "cut {cut}");
                if j.view.recovery.pending_request {
                    assert!(resolved.is_none());
                    assert!(j.resolve().is_some());
                    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::Ok);
                }
                let v = j.read().unwrap();
                assert!(v.has_result && !v.recovery.pending_request);
                assert_eq!(v.result.outcome, OwnerOutcome::Uncertain);
                assert_eq!(v.result.requested, CONFIG.max_portions);
                assert!(j.resolve().is_none() && !reserve(&mut j, rest + 1));
                assert!(reserve(&mut j, rest + 2));
            }
        }
    }
}

/// The cost of reading a checksum failure beside a pending reservation as
/// that reservation: flash corruption of a committed result makes the next
/// boot resolve the same feed again, as uncertain after its first result.
/// Its ID stays consumed, so it is never fed as new.
#[test]
fn a_corrupted_result_beside_its_own_reservation_resolves_that_feed_again() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, false);
    assert!(reserve(&mut j, 42) && finish(&mut j, true, true));
    assert_eq!(j.view.result.outcome, OwnerOutcome::Done);
    // Slot 0 holds the Done result, slot 1 its own reservation.

    f.bytes[0][42] ^= 1;
    let mut j = rebooted(&mut f, &CONFIG);
    assert_eq!(j.view.recovery.high_water_id, 42);
    assert!(j.resolve().is_some());
    assert_resolved(&j, 3);
    assert!(!reserve(&mut j, 42) && reserve(&mut j, 43));
}

/// The rotor's phase is never stored: it follows from the last result, so
/// the record layout stays as an older build reads it.
#[test]
fn the_last_result_gives_the_rotor_phase() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, false);
    assert_eq!(j.last_result(), None, "genesis holds no result");

    let phase = |j: &FeedJournal<FlashIo>| {
        j.last_result().map_or(RotorPhase::Unknown, |result| {
            RotorPhase::after(RotorPhase::Unknown, &result)
        })
    };
    assert!(reserve(&mut j, 1));
    assert_eq!(j.last_result(), None, "a pending reservation is no result");
    assert!(finish(&mut j, true, true));
    assert_eq!(phase(&j), RotorPhase::AtMark);

    assert!(reserve(&mut j, 2) && finish(&mut j, false, true));
    assert_eq!(phase(&j), RotorPhase::Unknown);

    // Reopened, the journal says the same, with no new field on flash.
    let config = j.config;
    assert_eq!(reopen(&mut f, &mut j, &config), JournalError::Ok);
    assert_eq!(phase(&j), RotorPhase::Unknown);
    assert!(reserve(&mut j, 3));
    assert_eq!(reopen(&mut f, &mut j, &config), JournalError::Ok);
    assert_eq!(
        j.resolve().map(|r| r.outcome),
        Some(OwnerOutcome::Uncertain)
    );
    assert_eq!(phase(&j), RotorPhase::Unknown);
}

#[test]
fn a_feed_that_never_moved_keeps_the_phase() {
    let moved = |outcome, motion_possible| OwnerResult {
        outcome,
        motion_possible,
        ..OwnerResult::default()
    };
    let at = RotorPhase::AtMark;
    let unknown = RotorPhase::Unknown;

    assert_eq!(
        RotorPhase::after(unknown, &moved(OwnerOutcome::Done, true)),
        at
    );
    assert_eq!(
        RotorPhase::after(at, &moved(OwnerOutcome::Expired, false)),
        at
    );
    assert_eq!(
        RotorPhase::after(unknown, &moved(OwnerOutcome::Failed, false)),
        unknown
    );
    for outcome in [
        OwnerOutcome::Cancelled,
        OwnerOutcome::Failed,
        OwnerOutcome::Uncertain,
    ] {
        assert_eq!(RotorPhase::after(at, &moved(outcome, true)), unknown);
    }
}
