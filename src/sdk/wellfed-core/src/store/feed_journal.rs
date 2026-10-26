//! Two-slot alternating feed journal with CRC-32 and a commit marker.
//!
//! The on-flash v1 record is 128 bytes and must stay byte-identical:
//! magic `PLFJNL01`, version, record size, type, mode, direction, flags,
//! sequence, binding, high-water ID, result counts, outcome, reason,
//! max portions, a zero span, CRC-32/ISO-HDLC of bytes 0..120 at 120 and
//! the `CMIT` commit marker at 124.
//!
//! A reservation a reset interrupted is resolved at the next boot by one
//! more result record, outcome [`OwnerOutcome::Uncertain`]: same layout,
//! same high-water ID. Its write is the same torn-write-safe sequence as any
//! record, and `open` reads every torn state of it as the pending
//! reservation, which resolves again, so a power cut never faults it and
//! never resolves it twice. Flash corruption can: a committed result that
//! later fails its checksum beside its own reservation reads as that
//! reservation, so the next boot resolves the feed again (an uncertain
//! result after its first), never as a new feed.

use core::ffi::c_uint;

use crate::feed::controller::{FeedReason, FeedRequest};
use crate::feed::progress::{BowlMode, Direction};
use crate::feed::storage::{OwnerOutcome, OwnerRecovery, OwnerResult, OwnerStorage};
use crate::store::slot::{
    Bound, COMMIT, COMMIT_AT, COMMIT_OFFSET, CRC_AT, LENGTH_AT, RECORD_BYTES, RECORD_LEN, SlotIo,
    TAIL_CHUNK, VERSION_AT, crc32, get_u16, get_u32, get_u64, put_u16, put_u32, put_u64,
};

const GENESIS: u8 = 0;
const INTENT: u8 = 1;
const RESULT: u8 = 2;

const MAGIC: [u8; 8] = *b"PLFJNL01";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalConfig {
    /// Nonzero installation/configuration binding, NOT an authentication key.
    /// Change it with incompatible board/configuration changes; do not reuse
    /// it to reset IDs. Reinitialization requires a separate recovery procedure.
    pub binding: u64,
    pub mode: BowlMode,
    pub max_portions: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum JournalError {
    #[default]
    Ok = 0,
    Argument = 1,
    Empty = 2,
    Io = 3,
    Corrupt = 4,
    State = 5,
    Exhausted = 6,
    Worn = 7,
}

impl JournalError {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

/// Records written before new reservations are refused: one unit erase per
/// record, alternating, so about 50 000 erases per unit, half a typical NOR
/// part's 100 000-cycle rating. Finishing an existing reservation is always
/// allowed, so a request is never stranded pending.
pub const WEAR_LIMIT: u64 = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalView {
    pub recovery: OwnerRecovery,
    pub result: OwnerResult,
    pub sequence: u64,
    pub has_result: bool,
}

/// Implementation-owned members. No concurrent access or native-struct dumps.
#[derive(Clone, Copy, Debug)]
pub struct FeedJournal<I> {
    io: Bound<I>,
    pub config: JournalConfig,
    pub view: JournalView,
    pub error: JournalError,
    pub slot: c_uint,
    pub opened: bool,
    pub reserved_here: bool,
}

impl<I> Default for FeedJournal<I> {
    fn default() -> Self {
        Self {
            io: Bound::NONE,
            config: JournalConfig::default(),
            view: JournalView::default(),
            error: JournalError::default(),
            slot: 0,
            opened: false,
            reserved_here: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Erased,
    Valid,
    /// No commit marker: a cut program, or an erase cut over the marker.
    Dirty,
    /// Committed, but failing its checksum: an erase cut over the record.
    Torn,
    /// A checked record that does not decode, or a unit with bytes past it.
    Invalid,
}

fn all(p: &[u8], value: u8) -> bool {
    p.iter().all(|&b| b == value)
}

fn phase_valid(c: &JournalConfig, p: &OwnerRecovery) -> bool {
    c.mode != BowlMode::Single || (p.direction == Direction::D0 && p.phase_known)
}

fn same_phase(a: &OwnerRecovery, b: &OwnerRecovery) -> bool {
    a.direction == b.direction
        && a.credit_next_event == b.credit_next_event
        && a.phase_known == b.phase_known
}

fn result_valid(c: &JournalConfig, v: &JournalView) -> bool {
    let r = &v.result;
    if r.outcome == OwnerOutcome::Pending
        || r.completed > r.requested
        || (!r.motion_possible && r.completed != 0)
    {
        return false;
    }

    if r.outcome == OwnerOutcome::Done {
        return r.completed == r.requested
            && r.motion_possible
            && r.reason == FeedReason::Done
            && v.recovery.phase_known;
    }

    if c.mode == BowlMode::Double && r.motion_possible && v.recovery.phase_known {
        return false;
    }
    if r.outcome == OwnerOutcome::Cancelled {
        return r.reason == FeedReason::Cancelled;
    }
    if r.outcome == OwnerOutcome::Expired {
        return !r.motion_possible && r.reason == FeedReason::TotalTimeout;
    }
    if r.outcome == OwnerOutcome::Uncertain {
        // Nothing was observed: the drive may have run, and no portion counts.
        return r.motion_possible && r.completed == 0 && r.reason == FeedReason::None;
    }
    r.reason != FeedReason::Done && r.reason != FeedReason::Cancelled
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
            && (c.mode != BowlMode::Double || next.recovery.phase_known);
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
    if next.result.outcome == OwnerOutcome::Done && c.mode == BowlMode::Double {
        if next.result.requested & 1 == 0 {
            expected.direction = expected.direction.toggled();
        }
        expected.credit_next_event = true;
    } else if next.result.motion_possible && c.mode == BowlMode::Double {
        expected.phase_known = false;
    }

    same_phase(&expected, &next.recovery)
}

impl<I: SlotIo> FeedJournal<I> {
    fn encode(&self, v: &JournalView) -> [u8; RECORD_BYTES] {
        let mut p = [0u8; RECORD_BYTES];
        p[..8].copy_from_slice(&MAGIC);
        put_u16::<VERSION_AT, _>(&mut p, 1);
        put_u16::<LENGTH_AT, _>(&mut p, RECORD_LEN);

        p[12] = if v.recovery.pending_request {
            INTENT
        } else if v.has_result {
            RESULT
        } else {
            GENESIS
        };
        p[13] = self.config.mode.byte();
        p[14] = v.recovery.direction.byte();
        p[15] = u8::from(v.recovery.credit_next_event)
            | (u8::from(v.recovery.phase_known) << 1)
            | (u8::from(v.result.motion_possible) << 2);

        put_u64::<16, _>(&mut p, v.sequence);
        put_u64::<24, _>(&mut p, self.config.binding);
        put_u64::<32, _>(&mut p, v.recovery.high_water_id);

        put_u16::<40, _>(&mut p, v.result.requested);
        put_u16::<42, _>(&mut p, v.result.completed);
        p[44] = v.result.outcome.byte();
        p[45] = v.result.reason.byte();
        put_u16::<46, _>(&mut p, self.config.max_portions);

        let checksum = crc32(&p[..CRC_AT]);
        put_u32::<CRC_AT, _>(&mut p, checksum);
        p[COMMIT_AT..].copy_from_slice(&COMMIT);

        p
    }

    fn decode(&self, p: &[u8; RECORD_BYTES]) -> Option<JournalView> {
        if p[..8] != MAGIC
            || get_u16::<VERSION_AT, _>(p) != 1
            || usize::from(get_u16::<LENGTH_AT, _>(p)) != RECORD_BYTES
            || p[12] > RESULT
            || p[13] != self.config.mode.byte()
            || p[15] > 7
            || get_u64::<24, _>(p) != self.config.binding
            || get_u16::<46, _>(p) != self.config.max_portions
            || !all(&p[48..CRC_AT], 0)
            || get_u32::<CRC_AT, _>(p) != crc32(&p[..CRC_AT])
        {
            return None;
        }

        let v = JournalView {
            sequence: get_u64::<16, _>(p),
            has_result: p[12] == RESULT,
            recovery: OwnerRecovery {
                high_water_id: get_u64::<32, _>(p),
                direction: Direction::from_byte(p[14])?,
                credit_next_event: p[15] & 1 != 0,
                phase_known: p[15] & 2 != 0,
                pending_request: p[12] == INTENT,
            },
            result: OwnerResult {
                id: get_u64::<32, _>(p),
                requested: get_u16::<40, _>(p),
                completed: get_u16::<42, _>(p),
                outcome: OwnerOutcome::from_byte(p[44])?,
                reason: FeedReason::from_byte(p[45])?,
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
                && v.result.outcome == OwnerOutcome::Pending
                && v.result.reason == FeedReason::None
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
                && v.result.outcome == OwnerOutcome::Pending
                && v.result.reason == FeedReason::None
                && !v.result.motion_possible
                && (self.config.mode != BowlMode::Double || v.recovery.phase_known)
        } else {
            v.sequence & 1 != 0 && result_valid(&self.config, &v)
        };

        valid.then_some(v)
    }

    /// Reset to an argument error, then adopt `io`/`config` if they are usable.
    fn setup(&mut self, io: I, config: JournalConfig) -> bool {
        *self = Self {
            error: JournalError::Argument,
            ..Self::default()
        };

        let unit = io.unit_bytes();
        if (unit as usize) < RECORD_BYTES
            || unit > 65536
            || !unit.is_multiple_of(4)
            || config.binding == 0
            || config.max_portions == 0
        {
            return false;
        }

        self.io = Bound::new(io);
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

        let mut offset = u32::from(RECORD_LEN);
        while offset < self.io.unit_bytes() {
            let chunk = (self.io.unit_bytes() - offset).min(TAIL_CHUNK);
            let size = chunk as usize;
            if !self.io.read(slot, offset, &mut tail[..size]) {
                return None;
            }
            if !all(&tail[..size], u8::MAX) {
                return Some((Scan::Invalid, None));
            }
            offset += chunk;
        }

        if all(&record, u8::MAX) {
            return Some((Scan::Erased, None));
        }
        if record[COMMIT_AT..] != COMMIT {
            return Some((Scan::Dirty, None));
        }
        if get_u32::<CRC_AT, _>(&record) != crc32(&record[..CRC_AT]) {
            return Some((Scan::Torn, None));
        }

        Some(match self.decode(&record) {
            Some(view) => (Scan::Valid, Some(view)),
            None => (Scan::Invalid, None),
        })
    }

    /// Empty/corrupt storage is never automatically initialized. Pending
    /// recovery is readable but cannot be reserved, finished or resumed here.
    pub fn open(&mut self, io: I, config: JournalConfig) -> JournalError {
        if !self.setup(io, config) {
            return JournalError::Argument;
        }

        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            return self.fail(JournalError::Io);
        };
        let views = [a.1.unwrap_or_default(), b.1.unwrap_or_default()];
        let (a, b) = (a.0, b.0);

        if a == Scan::Erased && b == Scan::Erased {
            return self.fail(JournalError::Empty);
        }
        if a != Scan::Valid && b != Scan::Valid {
            return self.fail(JournalError::Corrupt);
        }

        let mut slot = if a == Scan::Valid { 0 } else { 1 };
        if a == Scan::Valid && b == Scan::Valid {
            slot = if views[0].sequence > views[1].sequence {
                0
            } else {
                1
            };
            if !transition(&config, &views[1 - slot], &views[slot]) {
                return self.fail(JournalError::Corrupt);
            }
        } else {
            let other = [a, b][1 - slot];
            // Falling back to idle could forget a newer, ambiguous reservation.
            // Beside a pending one, the other unit holds an older record or
            // that reservation's own result, which a cut erase or program
            // leaves dirty or torn: the reservation stands, to be finished or
            // resolved again. A record that checks but does not decode is
            // never a cut's, and faults whatever it sits beside.
            let cut = matches!(other, Scan::Dirty | Scan::Torn);
            if other == Scan::Invalid || (cut && !views[slot].recovery.pending_request) {
                return self.fail(JournalError::Corrupt);
            }
        }

        self.slot = c_uint::from(slot == 1);
        self.view = views[slot];
        self.opened = true;
        self.error = JournalError::Ok;
        self.error
    }

    fn write_record(&mut self, slot: c_uint, next: &JournalView, erase: bool) -> bool {
        let bytes = self.encode(next);
        let mut checked = [0u8; RECORD_BYTES];
        // Program everything but the marker, verify it, then commit: a torn write
        // leaves an uncommitted record that `scan` reports as dirty.
        let ok = (!erase || self.io.erase(slot))
            && matches!(self.scan(slot), Some((Scan::Erased, _)))
            && self.io.program(slot, 0, &bytes[..COMMIT_AT])
            && self.io.read(slot, 0, &mut checked)
            && bytes[..COMMIT_AT] == checked[..COMMIT_AT]
            && all(&checked[COMMIT_AT..], u8::MAX)
            && self.io.program(slot, COMMIT_OFFSET, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;

        if !ok {
            self.fail(JournalError::Io);
            return false;
        }

        self.view = *next;
        self.slot = slot;
        self.opened = true;
        self.error = JournalError::Ok;
        true
    }

    /// Explicit first provisioning only: both units must already be entirely
    /// erased. Does not erase existing data or accept a nonzero/reset ID
    /// history. Mechanical phase must come from verified board evidence.
    pub fn create(
        &mut self,
        io: I,
        config: JournalConfig,
        direction: Direction,
        credit_next_event: bool,
        phase_known: bool,
    ) -> JournalError {
        if !self.setup(io, config) {
            return JournalError::Argument;
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
            return self.fail(JournalError::Io);
        };
        if a.0 != Scan::Erased || b.0 != Scan::Erased {
            return self.fail(JournalError::State);
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
        if view.recovery.pending_request && self.config.mode == BowlMode::Double {
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
            || (self.config.mode == BowlMode::Double && !request.phase_known)
        {
            return false;
        }

        if v.sequence == u64::MAX {
            self.fail(JournalError::Exhausted);
            return false;
        }
        if v.sequence >= WEAR_LIMIT {
            // Refuse without closing: the recovery state stays readable.
            self.error = JournalError::Worn;
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
            || result.outcome == OwnerOutcome::Uncertain
        {
            return false;
        }

        let next = JournalView {
            recovery: *phase,
            result: *result,
            ..self.view
        };
        self.append(next)
    }

    /// Resolve the reservation an earlier boot left pending as uncertain,
    /// keeping its ID as the high-water mark: the result, once durable.
    /// Interrupted double-bowl motion leaves the phase unknown, as `read`
    /// reports it. The reservation cannot have been made this boot: a live
    /// one has an owner that will finish it.
    pub fn resolve(&mut self) -> Option<OwnerResult> {
        if !self.opened || self.reserved_here || !self.view.recovery.pending_request {
            return None;
        }

        let mut next = JournalView {
            result: OwnerResult {
                id: self.view.recovery.high_water_id,
                requested: self.view.result.requested,
                completed: 0,
                outcome: OwnerOutcome::Uncertain,
                reason: FeedReason::None,
                motion_possible: true,
            },
            ..self.view
        };
        next.recovery.pending_request = false;
        if self.config.mode == BowlMode::Double {
            next.recovery.phase_known = false;
        }

        self.append(next).then_some(next.result)
    }

    /// Write `next` as the result of the pending reservation.
    fn append(&mut self, next: JournalView) -> bool {
        if self.view.sequence == u64::MAX {
            self.fail(JournalError::Exhausted);
            return false;
        }

        let next = JournalView {
            sequence: self.view.sequence + 1,
            has_result: true,
            ..next
        };

        if !transition(&self.config, &self.view, &next)
            || !self.write_record(1 - self.slot, &next, true)
        {
            return false;
        }
        self.reserved_here = false;
        true
    }
}

impl<I> FeedJournal<I> {
    /// Point a copy of this journal at a copy of its flash, keeping the
    /// journal's state: for tests that branch one history.
    #[doc(hidden)]
    pub fn rebind(&mut self, io: I) {
        self.io = Bound::new(io);
    }
}

impl<I: SlotIo> OwnerStorage for FeedJournal<I> {
    fn recover(&mut self, recovery: &mut OwnerRecovery) -> bool {
        match self.read() {
            Some(view) => {
                *recovery = view.recovery;
                true
            }
            None => false,
        }
    }

    fn reserve(&mut self, request: &FeedRequest) -> bool {
        FeedJournal::reserve(self, request)
    }

    fn finish(&mut self, result: &OwnerResult, phase: &OwnerRecovery) -> bool {
        FeedJournal::finish(self, result, phase)
    }

    fn resolve(&mut self) -> Option<OwnerResult> {
        FeedJournal::resolve(self)
    }
}
