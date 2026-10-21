use petlibro_core::feed_controller::{FeedReason, FeedRequest};
use petlibro_core::feed_journal::{FeedJournal, JournalConfig, JournalError, RECORD_BYTES};
use petlibro_core::feed_progress::{BowlMode, Direction};
use petlibro_core::feed_storage::{OwnerOutcome, OwnerResult};
use petlibro_testkit::journal_flash::{FakeJournalFlash, UNIT};

const CONFIG: JournalConfig = JournalConfig {
    binding: 0x91a5_7320_ae11_c008,
    mode: BowlMode::SINGLE,
    max_portions: 3,
};

fn request(j: &FeedJournal, id: u64) -> FeedRequest {
    FeedRequest {
        id,
        portions: 2,
        mode: j.config.mode,
        direction: j.view.recovery.direction,
        credit_next_event: j.view.recovery.credit_next_event,
        phase_known: j.view.recovery.phase_known,
    }
}

fn reserve(j: &mut FeedJournal, id: u64) -> bool {
    let r = request(j, id);
    j.reserve(&r)
}

fn finish(j: &mut FeedJournal, done: bool, motion: bool) -> bool {
    let mut phase = j.view.recovery;
    phase.pending_request = false;
    let mut result = j.view.result;
    result.completed = if done { result.requested } else { 0 };
    result.outcome = if done {
        OwnerOutcome::DONE
    } else {
        OwnerOutcome::CANCELLED
    };
    result.reason = if done {
        FeedReason::DONE
    } else {
        FeedReason::CANCELLED
    };
    result.motion_possible = motion;
    if j.config.mode == BowlMode::DOUBLE {
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

fn create(f: &mut FakeJournalFlash, dual: bool) -> FeedJournal {
    *f = FakeJournalFlash::default();
    let config = JournalConfig {
        mode: if dual {
            BowlMode::DOUBLE
        } else {
            BowlMode::SINGLE
        },
        ..CONFIG
    };
    let mut j = FeedJournal::default();
    // SAFETY: each test keeps `f` in place while the journal uses it.
    assert_eq!(
        unsafe { j.create(f.io(), config, Direction::D0, true, true) },
        JournalError::OK
    );
    j
}

fn reopen(f: &mut FakeJournalFlash, j: &mut FeedJournal, config: &JournalConfig) -> JournalError {
    f.budget = -1;
    f.fail_call = 0;
    // SAFETY: as for `create`.
    unsafe { j.open(f.io(), *config) }
}

/// Point a copied journal at a copied flash.
fn bind(j: &mut FeedJournal, f: &mut FakeJournalFlash) {
    j.io = f.io();
}

#[test]
fn lifecycle() {
    let mut f = FakeJournalFlash::default();
    let mut j = FeedJournal::default();
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::EMPTY);
    assert_eq!(f.mutations, 0);
    let mut j = create(&mut f, false);
    assert!(j.view.sequence == 1 && j.view.recovery.high_water_id == 0);
    assert!(reserve(&mut j, 0x1_0000_0001));
    assert!(j.view.sequence == 2 && j.view.recovery.pending_request);
    assert!(!reserve(&mut j, 0x1_0000_0002));
    assert!(finish(&mut j, true, true));
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::OK);
    let v = j.read().unwrap();
    assert!(v.has_result && !v.recovery.pending_request);
    assert!(v.result.id == 0x1_0000_0001 && v.result.completed == 2);
    assert!(!reserve(&mut j, v.result.id) && !reserve(&mut j, 1));
    assert!(reserve(&mut j, 0x1_0000_0010) && finish(&mut j, false, false));
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::OK);
    assert!(j.view.sequence == 5 && j.view.result.outcome == OwnerOutcome::CANCELLED);
    let writes = f.mutations;
    assert_eq!(
        unsafe { j.create(f.io(), CONFIG, Direction::D0, true, true) },
        JournalError::STATE
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
    assert_eq!(reopen(&mut f, &mut rebooted, &c), JournalError::OK);
    let storage = rebooted.storage();
    // SAFETY: `rebooted` stays in place while its storage callbacks run.
    let phase = unsafe { storage.recover() }.unwrap();
    assert!(phase.pending_request && !phase.phase_known);
    assert_eq!(phase.high_water_id, 10);
    assert!(!reserve(&mut rebooted, 11) && !finish(&mut rebooted, true, true));
    bind(&mut j, &mut f);
    assert!(finish(&mut j, true, true));
    assert!(j.view.recovery.direction == Direction::D1 && j.view.recovery.credit_next_event);
    assert!(reserve(&mut j, 11) && finish(&mut j, false, true));
    assert_eq!(reopen(&mut f, &mut j, &c), JournalError::OK);
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
    r.mode = BowlMode::DOUBLE;
    assert!(!j.reserve(&r));
    r.mode = BowlMode::SINGLE;
    r.direction = Direction::D1;
    assert!(!j.reserve(&r));
    assert!(f.mutations == writes && reserve(&mut j, 7));
    let mut phase = j.view.recovery;
    phase.pending_request = false;
    let mut result = OwnerResult {
        id: 7,
        requested: 2,
        completed: 2,
        outcome: OwnerOutcome::DONE,
        reason: FeedReason::DONE,
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
    let mut io = f.io();
    io.unit_bytes = 127;
    assert_eq!(unsafe { j.open(io, CONFIG) }, JournalError::ARGUMENT);
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
                if e == JournalError::OK {
                    // Only safe fallback is an unresolved intent, never idle.
                    assert!(j.view.recovery.pending_request && j.view.recovery.high_water_id == 41);
                    assert!(!reserve(&mut j, 42) && !finish(&mut j, true, true));
                } else {
                    assert_eq!(e, JournalError::CORRUPT);
                }
            }
        }
    }
    let mut f = original.clone();
    f.bytes[0][200] = 0;
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::CORRUPT);
    let c = JournalConfig {
        binding: CONFIG.binding + 1,
        ..CONFIG
    };
    assert_eq!(reopen(&mut original, &mut j, &c), JournalError::CORRUPT);
    let c = JournalConfig {
        mode: BowlMode::DOUBLE,
        ..CONFIG
    };
    assert_eq!(reopen(&mut original, &mut j, &c), JournalError::CORRUPT);
    let c = JournalConfig {
        max_portions: CONFIG.max_portions + 1,
        ..CONFIG
    };
    assert_eq!(reopen(&mut original, &mut j, &c), JournalError::CORRUPT);
}

fn byte_cuts(completing: bool, reused: bool) {
    let mut original = FakeJournalFlash::default();
    let mut original_j = create(&mut original, false);
    if reused {
        assert!(reserve(&mut original_j, 9) && finish(&mut original_j, true, true));
    }
    if completing {
        assert!(reserve(&mut original_j, 42));
    }
    for cut in 0..=(UNIT + RECORD_BYTES + 1) as i64 {
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
        if e == JournalError::OK {
            assert!(
                j.view.recovery.high_water_id == 42
                    || (!completing && j.view.recovery.high_water_id == if reused { 9 } else { 0 })
            );
            if completing {
                assert!(
                    j.view.recovery.pending_request
                        || (j.view.has_result && j.view.result.outcome == OwnerOutcome::DONE)
                );
            }
            if succeeded {
                assert!(
                    j.view.recovery.high_water_id == 42
                        && j.view.recovery.pending_request != completing
                );
            }
        } else {
            assert!(!succeeded && e == JournalError::CORRUPT);
        }
    }
}

#[test]
fn byte_cut_recovery() {
    byte_cuts(false, false);
    byte_cuts(false, true);
    byte_cuts(true, false);
    byte_cuts(true, true);
}

#[test]
fn create_cuts() {
    for cut in 0..=(RECORD_BYTES + 1) as i64 {
        let mut f = FakeJournalFlash {
            budget: cut,
            ..FakeJournalFlash::default()
        };
        let mut j = FeedJournal::default();
        let before = unsafe { j.create(f.io(), CONFIG, Direction::D0, true, true) };
        let after = reopen(&mut f, &mut j, &CONFIG);
        if after == JournalError::OK {
            assert!(j.view.sequence == 1 && j.view.recovery.high_water_id == 0);
        } else {
            assert!(
                before != JournalError::OK
                    && (after == JournalError::EMPTY || after == JournalError::CORRUPT)
            );
        }
    }
}

#[test]
fn io_failures() {
    let mut original = FakeJournalFlash::default();
    let original_j = create(&mut original, false);
    for completing in [false, true] {
        let mut base = original.clone();
        let mut source = original_j;
        bind(&mut source, &mut base);
        if completing {
            assert!(reserve(&mut source, 42));
        }
        // Includes reads before/after body and before/after final commit.
        for failure in 1..=20 {
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
                assert!(e == JournalError::OK && j.view.recovery.high_water_id == 42);
            }
            if completing && e == JournalError::OK {
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
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::OK);
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
        assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::CORRUPT);
    }
    let mut f = original.clone();
    f.bytes[0][16] = 5; // Valid odd result generation, but skips generation 3.
    repair_crc(&mut f.bytes[0]);
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::CORRUPT);
    let mut f = original.clone();
    f.bytes[1] = f.bytes[0];
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::CORRUPT);
}

#[test]
fn id_and_sequence_exhaustion() {
    let mut f = FakeJournalFlash::default();
    let mut j = create(&mut f, false);
    assert!(reserve(&mut j, u64::MAX) && finish(&mut j, true, true));
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::OK);
    assert!(j.view.recovery.high_water_id == u64::MAX && !reserve(&mut j, 1));
    // Synthesize the last adjacent generations, preserving valid checksums.
    for slot in 0..2 {
        f.bytes[slot][16..24].fill(u8::MAX);
        if slot == 1 {
            f.bytes[slot][16] = 0xfe;
        }
        repair_crc(&mut f.bytes[slot]);
    }
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::OK);
    assert_eq!(j.view.sequence, u64::MAX);
    // Restore an ordinary high-water to distinguish sequence from ID limit.
    for slot in 0..2 {
        f.bytes[slot][32..40].fill(0);
        f.bytes[slot][32] = 1;
        repair_crc(&mut f.bytes[slot]);
    }
    assert_eq!(reopen(&mut f, &mut j, &CONFIG), JournalError::OK);
    let writes = f.mutations;
    assert!(!reserve(&mut j, 2) && j.error == JournalError::EXHAUSTED && f.mutations == writes);
}
