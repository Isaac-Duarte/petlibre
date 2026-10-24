//! Recovery tests for the two-slot feed journal: lifecycle, phase recovery,
//! bit-flip corruption, binding mismatch, torn writes cut at every byte,
//! injected I/O failures, the pinned 128-byte v1 wire format, exhaustion and
//! the wear limit.
//!
//! The invariant under test: after any cut or failure, reopening yields the
//! committed state, an unresolved pending intent, or `CORRUPT`, never an idle
//! journal that has forgotten a reserved request ID.

use wellfed_core::feed::controller::{FeedReason, FeedRequest};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
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
                    // Only safe fallback is an unresolved intent, never idle.
                    assert!(j.view.recovery.pending_request && j.view.recovery.high_water_id == 41);
                    assert!(!reserve(&mut j, 42) && !finish(&mut j, true, true));
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

        let e = reopen(&mut f, &mut j, &CONFIG);
        if e == JournalError::Ok {
            assert!(
                j.view.recovery.high_water_id == 42
                    || (!completing && j.view.recovery.high_water_id == if reused { 9 } else { 0 })
            );
            if completing {
                assert!(
                    j.view.recovery.pending_request
                        || (j.view.has_result && j.view.result.outcome == OwnerOutcome::Done)
                );
            }
            if succeeded {
                assert!(
                    j.view.recovery.high_water_id == 42
                        && j.view.recovery.pending_request != completing
                );
            }
        } else {
            assert!(!succeeded && e == JournalError::Corrupt);
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
