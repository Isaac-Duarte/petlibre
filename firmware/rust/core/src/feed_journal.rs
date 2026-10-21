//! Two-slot alternating feed journal with CRC-32 and a commit marker.
//!
//! The on-flash v1 record is 128 bytes and must stay byte-identical:
//! magic `PLFJNL01`, version, record size, type, mode, direction, flags,
//! sequence, binding, high-water ID, result counts, outcome, reason,
//! max portions, a zero span, CRC-32/ISO-HDLC of bytes 0..120 at 120 and
//! the `CMIT` commit marker at 124.

use core::ffi::{c_uint, c_void};
use core::ptr;

use crate::feed_controller::{FeedReason, FeedRequest};
use crate::feed_progress::{BowlMode, Direction};
use crate::feed_storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};

pub const RECORD_BYTES: usize = 128;

const GENESIS: u8 = 0;
const INTENT: u8 = 1;
const RESULT: u8 = 2;
const MAGIC: [u8; 8] = *b"PLFJNL01";
const COMMIT: [u8; 4] = *b"CMIT";

pub type ReadFn = unsafe extern "C" fn(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool;
pub type ProgramFn = unsafe extern "C" fn(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool;
pub type EraseFn = unsafe extern "C" fn(context: *mut c_void, slot: c_uint) -> bool;

/// Two EXCLUSIVE, nonoverlapping erase units, assigned only after a complete
/// physical flash map/backup. This interface contains no physical addresses.
/// Reads must bypass/invalidate stale caches; writes return only after durable
/// completion. Program permits aligned 4-byte granules, only 1->0, splitting
/// physical pages as needed. Erase affects exactly one whole unit. All methods
/// must be bounded. False may mean partial OR completed I/O; never retry
/// blindly. The backend must independently enforce output inactivity and SDK
/// flash/IRQ synchronization.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct JournalIo {
    pub context: *mut c_void,
    pub unit_bytes: u32,
    pub read: Option<ReadFn>,
    pub program: Option<ProgramFn>,
    pub erase: Option<EraseFn>,
}

impl Default for JournalIo {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            unit_bytes: 0,
            read: None,
            program: None,
            erase: None,
        }
    }
}

#[allow(unsafe_code)]
impl JournalIo {
    // SAFETY (all three): `FeedJournal::open`/`create` require the callbacks
    // to be sound for `context`; the buffers are live Rust slices.
    pub(crate) fn read(&self, slot: c_uint, offset: u32, bytes: &mut [u8]) -> bool {
        self.read.is_some_and(|f| unsafe {
            f(
                self.context,
                slot,
                offset,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        })
    }

    pub(crate) fn program(&self, slot: c_uint, offset: u32, bytes: &[u8]) -> bool {
        self.program.is_some_and(|f| unsafe {
            f(
                self.context,
                slot,
                offset,
                bytes.as_ptr().cast(),
                bytes.len(),
            )
        })
    }

    pub(crate) fn erase(&self, slot: c_uint) -> bool {
        self.erase.is_some_and(|f| unsafe { f(self.context, slot) })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalConfig {
    /// Nonzero installation/configuration binding, NOT an authentication key.
    /// Change it with incompatible board/configuration changes; do not reuse
    /// it to reset IDs. Reinitialization requires a separate recovery procedure.
    pub binding: u64,
    pub mode: BowlMode,
    pub max_portions: u16,
}

c_enum!(JournalError { OK = 0, ARGUMENT = 1, EMPTY = 2, IO = 3, CORRUPT = 4, STATE = 5, EXHAUSTED = 6, WORN = 7 });

/// Records written before new reservations are refused: one unit erase per
/// record, alternating, so about 50 000 erases per unit, half a typical NOR
/// part's 100 000-cycle rating. Finishing an existing reservation is always
/// allowed, so a request is never stranded pending.
pub const WEAR_LIMIT: u64 = 100_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalView {
    pub recovery: OwnerRecovery,
    pub result: OwnerResult,
    pub sequence: u64,
    pub has_result: bool,
}

/// Implementation-owned members. No concurrent access or native-struct dumps.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FeedJournal {
    pub io: JournalIo,
    pub config: JournalConfig,
    pub view: JournalView,
    pub error: JournalError,
    pub slot: c_uint,
    pub opened: bool,
    pub reserved_here: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Erased,
    Valid,
    Dirty,
    Invalid,
}

fn put(p: &mut [u8], mut value: u64, count: usize) {
    for byte in &mut p[..count] {
        *byte = value as u8;
        value >>= 8;
    }
}

fn get(p: &[u8], count: usize) -> u64 {
    p[..count]
        .iter()
        .enumerate()
        .fold(0, |value, (i, &b)| value | (u64::from(b) << (i * 8)))
}

/// CRC-32/ISO-HDLC: reflected 0x04c11db7, init/xor-out all ones.
pub fn crc32(p: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for &byte in p {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xedb8_8320 } else { 0 };
        }
    }
    !value
}

fn all(p: &[u8], value: u8) -> bool {
    p.iter().all(|&b| b == value)
}

fn phase_valid(c: &JournalConfig, p: &OwnerRecovery) -> bool {
    p.direction.valid()
        && (c.mode != BowlMode::SINGLE || (p.direction == Direction::D0 && p.phase_known))
}

fn same_phase(a: &OwnerRecovery, b: &OwnerRecovery) -> bool {
    a.direction == b.direction
        && a.credit_next_event == b.credit_next_event
        && a.phase_known == b.phase_known
}

fn result_valid(c: &JournalConfig, v: &JournalView) -> bool {
    let r = &v.result;
    if r.outcome.0 < OwnerOutcome::DONE.0
        || r.outcome.0 > OwnerOutcome::FAILED.0
        || r.reason.0 > FeedReason::OUTPUT.0
        || r.completed > r.requested
        || (!r.motion_possible && r.completed != 0)
    {
        return false;
    }
    if r.outcome == OwnerOutcome::DONE {
        return r.completed == r.requested
            && r.motion_possible
            && r.reason == FeedReason::DONE
            && v.recovery.phase_known;
    }
    if c.mode == BowlMode::DOUBLE && r.motion_possible && v.recovery.phase_known {
        return false;
    }
    if r.outcome == OwnerOutcome::CANCELLED {
        return r.reason == FeedReason::CANCELLED;
    }
    if r.outcome == OwnerOutcome::EXPIRED {
        return !r.motion_possible && r.reason == FeedReason::TOTAL_TIMEOUT;
    }
    r.reason != FeedReason::DONE && r.reason != FeedReason::CANCELLED
}

fn transition(c: &JournalConfig, old: &JournalView, next: &JournalView) -> bool {
    if old.sequence == u64::MAX || next.sequence != old.sequence + 1 {
        return false;
    }
    if !old.recovery.pending_request {
        return next.recovery.pending_request
            && !next.has_result
            && next.recovery.high_water_id > old.recovery.high_water_id
            && same_phase(&old.recovery, &next.recovery)
            && (c.mode != BowlMode::DOUBLE || next.recovery.phase_known);
    }
    if next.recovery.pending_request
        || !next.has_result
        || next.recovery.high_water_id != old.recovery.high_water_id
        || next.result.requested != old.result.requested
        || !result_valid(c, next)
    {
        return false;
    }
    let mut expected = old.recovery;
    if next.result.outcome == OwnerOutcome::DONE && c.mode == BowlMode::DOUBLE {
        if next.result.requested & 1 == 0 {
            expected.direction = expected.direction.toggled();
        }
        expected.credit_next_event = true;
    } else if next.result.motion_possible && c.mode == BowlMode::DOUBLE {
        expected.phase_known = false;
    }
    same_phase(&expected, &next.recovery)
}

impl FeedJournal {
    fn encode(&self, v: &JournalView) -> [u8; RECORD_BYTES] {
        let mut p = [0u8; RECORD_BYTES];
        p[..8].copy_from_slice(&MAGIC);
        put(&mut p[8..], 1, 2);
        put(&mut p[10..], RECORD_BYTES as u64, 2);
        p[12] = if v.recovery.pending_request {
            INTENT
        } else if v.has_result {
            RESULT
        } else {
            GENESIS
        };
        p[13] = self.config.mode.0 as u8;
        p[14] = v.recovery.direction.0 as u8;
        p[15] = u8::from(v.recovery.credit_next_event)
            | (u8::from(v.recovery.phase_known) << 1)
            | (u8::from(v.result.motion_possible) << 2);
        put(&mut p[16..], v.sequence, 8);
        put(&mut p[24..], self.config.binding, 8);
        put(&mut p[32..], v.recovery.high_water_id, 8);
        put(&mut p[40..], v.result.requested.into(), 2);
        put(&mut p[42..], v.result.completed.into(), 2);
        p[44] = v.result.outcome.0 as u8;
        p[45] = v.result.reason.0 as u8;
        put(&mut p[46..], self.config.max_portions.into(), 2);
        let checksum = crc32(&p[..120]);
        put(&mut p[120..], checksum.into(), 4);
        p[124..].copy_from_slice(&COMMIT);
        p
    }

    fn decode(&self, p: &[u8; RECORD_BYTES]) -> Option<JournalView> {
        if p[..8] != MAGIC
            || get(&p[8..], 2) != 1
            || get(&p[10..], 2) != RECORD_BYTES as u64
            || p[12] > RESULT
            || p[13] != self.config.mode.0 as u8
            || p[14] > 1
            || p[15] > 7
            || get(&p[24..], 8) != self.config.binding
            || get(&p[46..], 2) != u64::from(self.config.max_portions)
            || !all(&p[48..120], 0)
            || get(&p[120..], 4) != u64::from(crc32(&p[..120]))
        {
            return None;
        }
        let v = JournalView {
            sequence: get(&p[16..], 8),
            has_result: p[12] == RESULT,
            recovery: OwnerRecovery {
                high_water_id: get(&p[32..], 8),
                direction: Direction(p[14].into()),
                credit_next_event: p[15] & 1 != 0,
                phase_known: p[15] & 2 != 0,
                pending_request: p[12] == INTENT,
            },
            result: OwnerResult {
                id: get(&p[32..], 8),
                requested: get(&p[40..], 2) as u16,
                completed: get(&p[42..], 2) as u16,
                outcome: OwnerOutcome(p[44].into()),
                reason: FeedReason(p[45].into()),
                motion_possible: p[15] & 4 != 0,
            },
        };
        if v.sequence == 0 || !phase_valid(&self.config, &v.recovery) {
            return None;
        }
        let valid = if p[12] == GENESIS {
            v.sequence == 1
                && v.recovery.high_water_id == 0
                && v.result.requested == 0
                && v.result.completed == 0
                && v.result.outcome == OwnerOutcome::PENDING
                && v.result.reason == FeedReason::NONE
                && !v.result.motion_possible
        } else if v.sequence == 1
            || v.recovery.high_water_id == 0
            || v.result.requested == 0
            || v.result.requested > self.config.max_portions
        {
            false
        } else if p[12] == INTENT {
            v.sequence & 1 == 0
                && v.result.completed == 0
                && v.result.outcome == OwnerOutcome::PENDING
                && v.result.reason == FeedReason::NONE
                && !v.result.motion_possible
                && (self.config.mode != BowlMode::DOUBLE || v.recovery.phase_known)
        } else {
            v.sequence & 1 != 0 && result_valid(&self.config, &v)
        };
        valid.then_some(v)
    }

    /// Reset to an argument error, then adopt `io`/`config` if they are usable.
    fn setup(&mut self, io: JournalIo, config: JournalConfig) -> bool {
        *self = Self {
            error: JournalError::ARGUMENT,
            ..Self::default()
        };
        if io.read.is_none()
            || io.program.is_none()
            || io.erase.is_none()
            || (io.unit_bytes as usize) < RECORD_BYTES
            || io.unit_bytes > 65536
            || !io.unit_bytes.is_multiple_of(4)
            || config.binding == 0
            || config.max_portions == 0
            || !config.mode.valid()
        {
            return false;
        }
        self.io = io;
        self.config = config;
        true
    }

    fn fail(&mut self, error: JournalError) -> JournalError {
        self.error = error;
        self.opened = false;
        self.reserved_here = false;
        error
    }

    /// Scan whole units so unrelated bytes cannot be mistaken for empty ownership.
    fn scan(&self, slot: c_uint) -> Option<(Scan, Option<JournalView>)> {
        let mut record = [0u8; RECORD_BYTES];
        let mut tail = [0u8; 64];
        if !self.io.read(slot, 0, &mut record) {
            return None;
        }
        let mut offset = RECORD_BYTES as u32;
        while offset < self.io.unit_bytes {
            let size = ((self.io.unit_bytes - offset) as usize).min(tail.len());
            if !self.io.read(slot, offset, &mut tail[..size]) {
                return None;
            }
            if !all(&tail[..size], u8::MAX) {
                return Some((Scan::Invalid, None));
            }
            offset += size as u32;
        }
        if all(&record, u8::MAX) {
            return Some((Scan::Erased, None));
        }
        if record[124..] != COMMIT {
            return Some((Scan::Dirty, None));
        }
        Some(match self.decode(&record) {
            Some(view) => (Scan::Valid, Some(view)),
            None => (Scan::Invalid, None),
        })
    }

    /// Empty/corrupt storage is never automatically initialized. Pending
    /// recovery is readable but cannot be reserved, finished or resumed here.
    ///
    /// # Safety
    /// `io`'s callbacks must be sound to call with its context for as long as
    /// this journal, or storage derived from it, is used.
    #[allow(unsafe_code)]
    pub unsafe fn open(&mut self, io: JournalIo, config: JournalConfig) -> JournalError {
        if !self.setup(io, config) {
            return JournalError::ARGUMENT;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            return self.fail(JournalError::IO);
        };
        let views = [a.1.unwrap_or_default(), b.1.unwrap_or_default()];
        let (a, b) = (a.0, b.0);
        if a == Scan::Erased && b == Scan::Erased {
            return self.fail(JournalError::EMPTY);
        }
        if a == Scan::Invalid || b == Scan::Invalid || (a != Scan::Valid && b != Scan::Valid) {
            return self.fail(JournalError::CORRUPT);
        }
        let mut slot = if a == Scan::Valid { 0 } else { 1 };
        if a == Scan::Valid && b == Scan::Valid {
            slot = if views[0].sequence > views[1].sequence {
                0
            } else {
                1
            };
            if !transition(&config, &views[1 - slot], &views[slot]) {
                return self.fail(JournalError::CORRUPT);
            }
        } else if (a == Scan::Dirty || b == Scan::Dirty) && !views[slot].recovery.pending_request {
            // Falling back to idle could forget a newer, ambiguous reservation.
            return self.fail(JournalError::CORRUPT);
        }
        self.slot = slot as c_uint;
        self.view = views[slot];
        self.opened = true;
        self.error = JournalError::OK;
        self.error
    }

    fn write_record(&mut self, slot: c_uint, next: &JournalView, erase: bool) -> bool {
        let bytes = self.encode(next);
        let mut checked = [0u8; RECORD_BYTES];
        let ok = (!erase || self.io.erase(slot))
            && matches!(self.scan(slot), Some((Scan::Erased, _)))
            && self.io.program(slot, 0, &bytes[..124])
            && self.io.read(slot, 0, &mut checked)
            && bytes[..124] == checked[..124]
            && all(&checked[124..], u8::MAX)
            && self.io.program(slot, 124, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;
        if !ok {
            self.fail(JournalError::IO);
            return false;
        }
        self.view = *next;
        self.slot = slot;
        self.opened = true;
        self.error = JournalError::OK;
        true
    }

    /// Explicit first provisioning only: both units must already be entirely
    /// erased. Does not erase existing data or accept a nonzero/reset ID
    /// history. Mechanical phase must come from verified board evidence.
    ///
    /// # Safety
    /// As for [`FeedJournal::open`].
    #[allow(unsafe_code)]
    pub unsafe fn create(
        &mut self,
        io: JournalIo,
        config: JournalConfig,
        direction: Direction,
        credit_next_event: bool,
        phase_known: bool,
    ) -> JournalError {
        if !self.setup(io, config) {
            return JournalError::ARGUMENT;
        }
        let next = JournalView {
            sequence: 1,
            recovery: OwnerRecovery {
                direction,
                credit_next_event,
                phase_known,
                ..OwnerRecovery::default()
            },
            ..JournalView::default()
        };
        if !phase_valid(&config, &next.recovery) {
            return self.error;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            return self.fail(JournalError::IO);
        };
        if a.0 != Scan::Erased || b.0 != Scan::Erased {
            return self.fail(JournalError::STATE);
        }
        self.write_record(0, &next, false);
        self.error
    }

    pub fn read(&self) -> Option<JournalView> {
        if !self.opened {
            return None;
        }
        let mut view = self.view;
        // Interrupted double-bowl motion does not establish a physical phase.
        if view.recovery.pending_request && self.config.mode == BowlMode::DOUBLE {
            view.recovery.phase_known = false;
        }
        Some(view)
    }

    pub fn reserve(&mut self, request: &FeedRequest) -> bool {
        let v = &self.view;
        if !self.opened
            || v.recovery.pending_request
            || request.id == 0
            || request.id <= v.recovery.high_water_id
            || request.portions == 0
            || request.portions > self.config.max_portions
            || request.mode != self.config.mode
            || request.direction != v.recovery.direction
            || request.credit_next_event != v.recovery.credit_next_event
            || request.phase_known != v.recovery.phase_known
            || (self.config.mode == BowlMode::DOUBLE && !request.phase_known)
        {
            return false;
        }
        if v.sequence == u64::MAX {
            self.fail(JournalError::EXHAUSTED);
            return false;
        }
        if v.sequence >= WEAR_LIMIT {
            // Refuse without closing: the recovery state stays readable.
            self.error = JournalError::WORN;
            return false;
        }
        let mut next = JournalView {
            sequence: v.sequence + 1,
            recovery: v.recovery,
            result: OwnerResult {
                id: request.id,
                requested: request.portions,
                ..OwnerResult::default()
            },
            has_result: false,
        };
        next.recovery.high_water_id = request.id;
        next.recovery.pending_request = true;
        if !self.write_record(1 - self.slot, &next, true) {
            return false;
        }
        self.reserved_here = true;
        true
    }

    pub fn finish(&mut self, result: &OwnerResult, phase: &OwnerRecovery) -> bool {
        if !self.opened
            || !self.reserved_here
            || !self.view.recovery.pending_request
            || phase.pending_request
            || phase.high_water_id != result.id
            || result.id != self.view.recovery.high_water_id
            || !phase_valid(&self.config, phase)
        {
            return false;
        }
        if self.view.sequence == u64::MAX {
            self.fail(JournalError::EXHAUSTED);
            return false;
        }
        let next = JournalView {
            sequence: self.view.sequence + 1,
            recovery: *phase,
            result: *result,
            has_result: true,
        };
        if !transition(&self.config, &self.view, &next)
            || !self.write_record(1 - self.slot, &next, true)
        {
            return false;
        }
        self.reserved_here = false;
        true
    }

    /// Owner-storage callbacks bound to this journal. The journal must not move
    /// or be accessed elsewhere while the returned storage is in use.
    pub fn storage(&mut self) -> OwnerStorage {
        OwnerStorage {
            context: ptr::from_mut(self).cast(),
            recover: Some(storage_recover),
            reserve: Some(storage_reserve),
            finish: Some(storage_finish),
        }
    }
}

// SAFETY (all three): `context` is the journal pointer installed by
// `FeedJournal::storage`, or null; the owner serializes all access.
#[allow(unsafe_code)]
unsafe extern "C" fn storage_recover(context: *mut c_void, recovery: *mut OwnerRecovery) -> bool {
    let (Some(journal), Some(recovery)) =
        (unsafe { context.cast::<FeedJournal>().as_ref() }, unsafe {
            recovery.as_mut()
        })
    else {
        return false;
    };
    match journal.read() {
        Some(view) => {
            *recovery = view.recovery;
            true
        }
        None => false,
    }
}

#[allow(unsafe_code)]
unsafe extern "C" fn storage_reserve(context: *mut c_void, request: *const FeedRequest) -> bool {
    match (unsafe { context.cast::<FeedJournal>().as_mut() }, unsafe {
        request.as_ref()
    }) {
        (Some(journal), Some(request)) => journal.reserve(request),
        _ => false,
    }
}

#[allow(unsafe_code)]
unsafe extern "C" fn storage_finish(
    context: *mut c_void,
    result: *const OwnerResult,
    phase: *const OwnerRecovery,
) -> bool {
    match (
        unsafe { context.cast::<FeedJournal>().as_mut() },
        unsafe { result.as_ref() },
        unsafe { phase.as_ref() },
    ) {
        (Some(journal), Some(result), Some(phase)) => journal.finish(result, phase),
        _ => false,
    }
}
