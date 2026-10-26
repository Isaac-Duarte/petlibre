//! The per-feed trace ring: the latest feeds' traces in storage sectors
//! 29-31, so the feeder keeps its own history of what each feed measured
//! across restarts.
//!
//! Each sector holds four 1 KiB slots, filled in order:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | `PLTR` |
//! | 4 | layout version, 1 |
//! | 5 | the summary's length |
//! | 6..8 | the body's length, little-endian |
//! | 8..12 | sequence number, little-endian, from 1 |
//! | 12..92 | the summary (`wellfed_proto::api::TraceSummary`, postcard, zero-padded) |
//! | 92..96 | CRC-32 of bytes 0..92 and the body |
//! | 96.. | the body ([`wellfed_proto::trace`]), at most 864 bytes |
//!
//! - **Writes** come only from the settings writer, under the write turn,
//!   after the feed is over and its outputs are stopped: the header in one
//!   program, then the body in another. A cut before both are in leaves a
//!   slot that is not blank and fails its checksum: it is skipped, and the
//!   next trace takes the slot after it.
//! - **Room:** when the newest sector is full, the oldest is erased and
//!   becomes the newest, forgetting its four traces before the erase
//!   starts. Two full sectors always remain, so at least the last 8 traces
//!   are kept and at most 12; each sector is erased once every 12 feeds.
//! - **Order:** sequence numbers rise by one per trace across restarts; on
//!   open the ring finds its newest valid trace and continues after it. A
//!   torn erase leaves its surviving traces valid under their old numbers,
//!   and anything else fails its checksum.
//! - **Reads** need no state: [`find`] scans the slots' headers for a feed
//!   and checks the whole trace's checksum, so the loop reads the ring
//!   directly from mapped flash while the writer owns the appends. A feed
//!   traced twice (its number reused after the journal was wiped) reads as
//!   the newer.

use core::ffi::c_uint;

use wellfed_core::store::slot::{Crc32, SlotIo};

use crate::feeder::platform::TraceRead;
use wellfed_proto::api::{Refusal, TraceSummary};
use wellfed_proto::trace::BODY_MAX;

/// One trace's slot.
pub const SLOT_BYTES: u32 = 1024;
/// The header before the body.
pub const HEADER_BYTES: usize = 96;
const _: () = assert!(HEADER_BYTES + BODY_MAX <= SLOT_BYTES as usize);
/// The sectors the ring is laid out for, and the most it tracks.
pub const MAX_UNITS: usize = 3;

const MAGIC: [u8; 4] = *b"PLTR";
const VERSION: u8 = 1;
const SUMMARY_AT: usize = 12;
const CRC_AT: usize = 92;
/// The bytes a read or a checksum takes at a time.
const CHUNK: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TraceError {
    #[error("the trace ring is not open")]
    Closed,
    #[error("a flash erase or program failed")]
    Io,
    #[error("the trace does not fit a slot")]
    TooLong,
}

/// One unit's slots as the scan found them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Unit {
    /// The first slot never programmed.
    free: u32,
    /// Valid traces, and the newest one's number.
    valid: u32,
    newest: Option<u32>,
}

/// The appending half, which the settings writer owns.
#[derive(Debug)]
pub struct TraceRing<I> {
    io: Option<I>,
    units: usize,
    per_unit: u32,
    table: [Unit; MAX_UNITS],
    /// The unit appends go to.
    head: usize,
    next_seq: u32,
}

impl<I> Default for TraceRing<I> {
    fn default() -> Self {
        Self::closed()
    }
}

impl<I> TraceRing<I> {
    pub const fn closed() -> Self {
        Self {
            io: None,
            units: 0,
            per_unit: 0,
            table: [Unit {
                free: 0,
                valid: 0,
                newest: None,
            }; MAX_UNITS],
            head: 0,
            next_seq: 1,
        }
    }

    pub const fn is_open(&self) -> bool {
        self.io.is_some()
    }

    /// Traces the ring holds.
    pub fn kept(&self) -> u32 {
        self.table[..self.units].iter().map(|unit| unit.valid).sum()
    }

    /// Traces the ring keeps at least: every unit's but the one the next
    /// wrap erases.
    pub fn capacity(&self) -> u32 {
        let units = u32::try_from(self.units.saturating_sub(1)).unwrap_or(0);
        units.saturating_mul(self.per_unit)
    }

    /// The newest trace's number, `None` before the first.
    pub const fn last_seq(&self) -> Option<u32> {
        match self.next_seq {
            1 => None,
            next => Some(next - 1),
        }
    }
}

impl<I: SlotIo> TraceRing<I> {
    /// Scan `units` erase units of `io` (2 to [`MAX_UNITS`]), which belong to
    /// the ring alone.
    pub fn open(&mut self, io: I, units: usize) {
        *self = Self::closed();
        let per_unit = io.unit_bytes() / SLOT_BYTES;
        if !(2..=MAX_UNITS).contains(&units) || per_unit == 0 {
            return;
        }
        self.units = units;
        self.per_unit = per_unit;

        for index in 0..units {
            let mut unit = Unit::default();
            for slot in 0..per_unit {
                if blank(&io, index, slot) {
                    // Traces fill a unit in order, so the first blank slot
                    // ends its traces.
                    break;
                }
                unit.free = slot + 1;
                if let Some(header) = valid(&io, index, slot) {
                    unit.valid += 1;
                    unit.newest = Some(unit.newest.map_or(header.seq, |n| n.max(header.seq)));
                }
            }
            self.table[index] = unit;
        }
        let newest = (0..units)
            .filter_map(|index| self.table[index].newest.map(|seq| (index, seq)))
            .max_by_key(|&(_, seq)| seq);
        if let Some((index, seq)) = newest {
            self.head = index;
            self.next_seq = seq.saturating_add(1);
        }
        self.io = Some(io);
    }

    /// Append one sealed trace.
    pub fn append(&mut self, summary: &TraceSummary, body: &[u8]) -> Result<u32, TraceError> {
        let io = self.io.as_ref().ok_or(TraceError::Closed)?;
        let body_len = u16::try_from(body.len())
            .ok()
            .filter(|_| body.len() <= BODY_MAX)
            .ok_or(TraceError::TooLong)?;
        let seq = self.next_seq;
        let mut header = [0u8; HEADER_BYTES];
        let summary_len = wellfed_proto::encode(summary, &mut header[SUMMARY_AT..CRC_AT])
            .map_err(|_| TraceError::TooLong)?;
        header[..4].copy_from_slice(&MAGIC);
        header[4] = VERSION;
        header[5] = u8::try_from(summary_len).map_err(|_| TraceError::TooLong)?;
        header[6..8].copy_from_slice(&body_len.to_le_bytes());
        header[8..12].copy_from_slice(&seq.to_le_bytes());
        let mut crc = Crc32::new();
        crc.update(&header[..CRC_AT]);
        crc.update(body);
        header[CRC_AT..].copy_from_slice(&crc.finish().to_le_bytes());

        if self.table[self.head].free >= self.per_unit {
            // The newest unit is full: the oldest becomes the newest. Its
            // traces are forgotten before the erase starts, and the head
            // moves only once it succeeds, so a failed erase is tried again
            // by the next append.
            let next = (self.head + 1) % self.units;
            self.table[next] = Unit {
                free: self.per_unit,
                ..Unit::default()
            };
            if !io.erase(unit_id(next)) {
                return Err(TraceError::Io);
            }
            self.table[next].free = 0;
            self.head = next;
        }

        let unit = &mut self.table[self.head];
        let offset = unit.free * SLOT_BYTES;
        unit.free += 1;
        if !io.program(unit_id(self.head), offset, &header)
            || !program_body(io, self.head, offset, body)
        {
            return Err(TraceError::Io);
        }
        unit.valid += 1;
        unit.newest = Some(seq);
        self.next_seq = seq.saturating_add(1);
        Ok(seq)
    }
}

/// One page of feed `feed`'s stored trace from `offset` into `out`, for
/// the device API: `NotFound` when the ring holds no trace of it,
/// `Invalid` for an offset past its body.
pub fn page<I: SlotIo>(
    io: &I,
    units: usize,
    feed: u32,
    offset: u16,
    out: &mut [u8],
) -> Result<TraceRead, Refusal> {
    let found = find(io, units, feed).ok_or(Refusal::NotFound)?;
    if offset > found.body_len {
        return Err(Refusal::Invalid);
    }
    let len = read_body(io, &found, offset, out).ok_or(Refusal::NotFound)?;
    Ok(TraceRead {
        summary: found.summary,
        stored: true,
        total: found.body_len,
        len,
    })
}

/// Program `body` after the header of the slot at `offset`. Flash takes
/// whole words, so a last partial word goes out padded with erased bytes,
/// which the checksum does not cover.
fn program_body<I: SlotIo>(io: &I, unit: usize, offset: u32, body: &[u8]) -> bool {
    let Some(at) = u32::try_from(HEADER_BYTES)
        .ok()
        .and_then(|header| offset.checked_add(header))
    else {
        return false;
    };
    let (words, tail) = body.split_at(body.len() & !3);
    if !words.is_empty() && !io.program(unit_id(unit), at, words) {
        return false;
    }
    if tail.is_empty() {
        return true;
    }
    let mut last = [u8::MAX; 4];
    last[..tail.len()].copy_from_slice(tail);
    let Some(at) = u32::try_from(words.len())
        .ok()
        .and_then(|len| at.checked_add(len))
    else {
        return false;
    };
    io.program(unit_id(unit), at, &last)
}

/// A valid trace's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub summary: TraceSummary,
    pub seq: u32,
    pub body_len: u16,
    unit: usize,
    slot: u32,
}

/// The newest valid trace of feed `feed` in the ring's `units` units.
pub fn find<I: SlotIo>(io: &I, units: usize, feed: u32) -> Option<Found> {
    let per_unit = io.unit_bytes() / SLOT_BYTES;
    let mut best: Option<Found> = None;
    for unit in 0..units.min(MAX_UNITS) {
        for slot in 0..per_unit {
            let Some(found) = header(io, unit, slot) else {
                continue;
            };
            if found.summary.feed == feed
                && best.is_none_or(|best| found.seq > best.seq)
                && checks(io, &found)
            {
                best = Some(found);
            }
        }
    }
    best
}

/// Copy the body of `found` from `offset` into `out`, as much as fits; the
/// bytes copied, `None` when the read failed or the trace no longer checks
/// (its unit was erased since it was found).
pub fn read_body<I: SlotIo>(io: &I, found: &Found, offset: u16, out: &mut [u8]) -> Option<usize> {
    let offset = usize::from(offset);
    let len = usize::from(found.body_len)
        .checked_sub(offset)?
        .min(out.len());
    let at = slot_offset(found.slot, HEADER_BYTES + offset)?;
    let out = out.get_mut(..len)?;
    if !io.read(unit_id(found.unit), at, out) {
        return None;
    }
    // Checked after the copy, so a trace erased while it was read is not
    // answered with another's bytes.
    header(io, found.unit, found.slot)
        .is_some_and(|again| again == *found && checks(io, found))
        .then_some(len)
}

/// The header of a slot, when its fields are well formed; the checksum is
/// not checked here.
fn header<I: SlotIo>(io: &I, unit: usize, slot: u32) -> Option<Found> {
    let mut header = [0u8; HEADER_BYTES];
    if !io.read(unit_id(unit), slot_offset(slot, 0)?, &mut header) || header[..4] != MAGIC {
        return None;
    }
    if header[4] != VERSION {
        return None;
    }
    let summary_len = usize::from(header[5]);
    let body_len = u16::from_le_bytes([header[6], header[7]]);
    let seq = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if usize::from(body_len) > BODY_MAX || SUMMARY_AT + summary_len > CRC_AT {
        return None;
    }
    let summary = wellfed_proto::decode(&header[SUMMARY_AT..SUMMARY_AT + summary_len]).ok()?;
    Some(Found {
        summary,
        seq,
        body_len,
        unit,
        slot,
    })
}

/// A slot's header, when its whole trace checks.
fn valid<I: SlotIo>(io: &I, unit: usize, slot: u32) -> Option<Found> {
    header(io, unit, slot).filter(|found| checks(io, found))
}

/// Whether the trace's checksum covers what the slot holds.
fn checks<I: SlotIo>(io: &I, found: &Found) -> bool {
    let unit = unit_id(found.unit);
    let mut header = [0u8; HEADER_BYTES];
    let Some(base) = slot_offset(found.slot, 0) else {
        return false;
    };
    if !io.read(unit, base, &mut header) {
        return false;
    }
    let mut crc = Crc32::new();
    crc.update(&header[..CRC_AT]);

    let mut chunk = [0u8; CHUNK];
    let len = usize::from(found.body_len);
    let mut at = 0;
    while at < len {
        let piece = &mut chunk[..CHUNK.min(len - at)];
        let Some(offset) = slot_offset(found.slot, HEADER_BYTES + at) else {
            return false;
        };
        if !io.read(unit, offset, piece) {
            return false;
        }
        crc.update(piece);
        at += piece.len();
    }
    let stored = [
        header[CRC_AT],
        header[CRC_AT + 1],
        header[CRC_AT + 2],
        header[CRC_AT + 3],
    ];
    crc.finish() == u32::from_le_bytes(stored)
}

/// Whether a slot is erased throughout.
fn blank<I: SlotIo>(io: &I, unit: usize, slot: u32) -> bool {
    let mut chunk = [0u8; CHUNK];
    let slot_bytes = usize::try_from(SLOT_BYTES).unwrap_or(0);
    (0..slot_bytes).step_by(CHUNK).all(|at| {
        slot_offset(slot, at).is_some_and(|offset| io.read(unit_id(unit), offset, &mut chunk))
            && chunk.iter().all(|&b| b == u8::MAX)
    })
}

fn slot_offset(slot: u32, within: usize) -> Option<u32> {
    slot.checked_mul(SLOT_BYTES)?
        .checked_add(u32::try_from(within).ok()?)
}

fn unit_id(index: usize) -> c_uint {
    c_uint::try_from(index).unwrap_or(c_uint::MAX)
}
