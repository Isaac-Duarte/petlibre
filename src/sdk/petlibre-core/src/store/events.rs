//! FW47's durable event ring: at least 7 days of the feeder's history in
//! storage sectors 13-24 ([resource budget]).
//!
//! Records are 32 bytes, appended in order through a ring of erase units:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..8 | sequence number, little-endian, from 1 |
//! | 8..12 | Unix seconds, 0 when the clock was invalid |
//! | 12..28 | the body (`petlibre_proto::api::EventBody`, postcard, zero-padded) |
//! | 28..32 | CRC-32 of bytes 0..28 |
//!
//! - **Order:** sequence numbers rise by one per record across reboots: on
//!   open the ring finds its newest valid record, and the next append takes
//!   the number after it. A torn record fails its CRC and is skipped; its
//!   number is reused, so the history has no hole.
//! - **Room:** when the newest unit is full the oldest is erased and
//!   becomes the newest, so `units - 1` units always hold history. With 12
//!   units of 4 KiB that is 1,408 records, 7 days at 200 a day.
//! - **Reads** find the first record after a cursor. A cursor older than the
//!   oldest record kept reads as a gap, so a client knows what it lost.
//!
//! A torn erase leaves a unit with arbitrary bits raised; its surviving old
//! records still check and keep their old numbers, and anything else fails
//! its CRC, so the ring's order never depends on where the cut fell.
//!
//! [resource budget]: ../../../../../docs/firmware/resource-budget.md

use core::ffi::c_uint;

use crate::store::slot::{JournalIo, crc32, get_u32, get_u64, put_u32, put_u64};

pub const RECORD_BYTES: usize = 32;
/// [`RECORD_BYTES`] as the I/O offsets count.
const RECORD_U32: u32 = 32;
const _: () = assert!(RECORD_U32 as usize == RECORD_BYTES);
/// The body's room in a record.
pub const BODY_BYTES: usize = 16;
const AT_AT: usize = 8;
const BODY_AT: usize = 12;
const CRC_AT: usize = 28;

/// Units the log tracks at most.
pub const MAX_UNITS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    /// Unix seconds, `None` when the clock was invalid.
    pub at: Option<u32>,
    pub body: [u8; BODY_BYTES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Record(Record),
    /// Records `from..=to` are gone.
    Gap {
        from: u64,
        to: u64,
    },
    /// Nothing after the cursor yet.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventError {
    /// Not opened, or opened on unusable I/O.
    Closed,
    /// A program or erase failed; the record may be torn, and the next
    /// append moves past it.
    Io,
}

/// One unit's records as the scan found them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Unit {
    /// The first and last valid sequence numbers, if any.
    span: Option<(u64, u64)>,
    /// The first slot never programmed.
    free: u32,
}

#[derive(Debug)]
pub struct EventLog {
    io: Option<JournalIo>,
    units: usize,
    per_unit: u32,
    table: [Unit; MAX_UNITS],
    /// The unit appends go to.
    head: usize,
    next_seq: u64,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::closed()
    }
}

impl EventLog {
    pub const fn closed() -> Self {
        Self {
            io: None,
            units: 0,
            per_unit: 0,
            table: [Unit {
                span: None,
                free: 0,
            }; MAX_UNITS],
            head: 0,
            next_seq: 1,
        }
    }

    /// Scan `units` erase units of `io` (2 to [`MAX_UNITS`]).
    ///
    /// # Safety
    /// `io`'s callbacks must be sound for its context for as long as the log
    /// is used, and its units must belong to the log alone.
    #[allow(unsafe_code)]
    pub unsafe fn open(&mut self, io: JournalIo, units: usize) {
        *self = Self::closed();
        let per_unit = io.unit_bytes / RECORD_U32;
        if !(2..=MAX_UNITS).contains(&units) || per_unit == 0 {
            return;
        }
        self.units = units;
        self.per_unit = per_unit;
        self.io = Some(io);

        for index in 0..units {
            self.table[index] = self.scan(index);
        }
        let newest = (0..units)
            .filter_map(|index| self.table[index].span.map(|(_, last)| (index, last)))
            .max_by_key(|&(_, last)| last);
        if let Some((index, last)) = newest {
            self.head = index;
            self.next_seq = last + 1;
        }
    }

    pub const fn is_open(&self) -> bool {
        self.io.is_some()
    }

    /// The newest record's number, `None` before the first.
    pub const fn last_seq(&self) -> Option<u64> {
        match self.next_seq {
            1 => None,
            next => Some(next - 1),
        }
    }

    /// The oldest record still kept.
    pub fn oldest_seq(&self) -> Option<u64> {
        self.table[..self.units]
            .iter()
            .filter_map(|unit| unit.span.map(|(first, _)| first))
            .min()
    }

    fn scan(&self, index: usize) -> Unit {
        let mut unit = Unit::default();
        for slot in 0..self.per_unit {
            let Some(bytes) = self.read_slot(index, slot) else {
                break;
            };
            if bytes == [u8::MAX; RECORD_BYTES] {
                // Appends fill a unit in order, so the first blank slot ends
                // its records.
                break;
            }
            unit.free = slot + 1;
            if let Some(record) = decode(&bytes) {
                unit.span = Some(match unit.span {
                    None => (record.seq, record.seq),
                    Some((first, last)) => (first.min(record.seq), last.max(record.seq)),
                });
            }
        }
        unit
    }

    fn read_slot(&self, index: usize, slot: u32) -> Option<[u8; RECORD_BYTES]> {
        let io = self.io.as_ref()?;
        let mut bytes = [0; RECORD_BYTES];
        io.read(unit_id(index), slot * RECORD_U32, &mut bytes)
            .then_some(bytes)
    }

    /// Append one record; its sequence number.
    pub fn append(&mut self, at: Option<u32>, body: &[u8; BODY_BYTES]) -> Result<u64, EventError> {
        let io = self.io.ok_or(EventError::Closed)?;
        if self.table[self.head].free >= self.per_unit {
            // The newest unit is full: the oldest becomes the newest.
            // Its records are forgotten before the erase starts, and the
            // head moves only once it succeeds, so a failed erase is simply
            // tried again by the next append.
            let next = (self.head + 1) % self.units;
            self.table[next] = Unit {
                span: None,
                free: self.per_unit,
            };
            if !io.erase(unit_id(next)) {
                return Err(EventError::Io);
            }
            self.table[next].free = 0;
            self.head = next;
        }

        let seq = self.next_seq;
        let record = encode(&Record {
            seq,
            at,
            body: *body,
        });
        let unit = &mut self.table[self.head];
        let slot = unit.free;
        unit.free += 1;
        if !io.program(unit_id(self.head), slot * RECORD_U32, &record) {
            return Err(EventError::Io);
        }
        unit.span = Some(unit.span.map_or((seq, seq), |(first, _)| (first, seq)));
        self.next_seq = seq + 1;
        Ok(seq)
    }

    /// The first record after `after` (`None`: from the oldest kept).
    pub fn read_after(&self, after: Option<u64>) -> Read {
        let Some(oldest) = self.oldest_seq() else {
            return Read::End;
        };
        let want = after.map_or(oldest, |after| after + 1);
        if want < oldest {
            return Read::Gap {
                from: want,
                to: oldest - 1,
            };
        }
        if want >= self.next_seq {
            return Read::End;
        }

        // The unit whose span holds `want`, then its records in order.
        for index in 0..self.units {
            let Some((first, last)) = self.table[index].span else {
                continue;
            };
            if !(first..=last).contains(&want) {
                continue;
            }
            let mut best: Option<Record> = None;
            for slot in 0..self.table[index].free {
                let Some(record) = self.read_slot(index, slot).and_then(|b| decode(&b)) else {
                    continue;
                };
                if record.seq == want {
                    return Read::Record(record);
                }
                if record.seq > want && best.is_none_or(|b| record.seq < b.seq) {
                    best = Some(record);
                }
            }
            if let Some(record) = best {
                return Read::Record(record);
            }
        }
        Read::End
    }
}

fn unit_id(index: usize) -> c_uint {
    c_uint::try_from(index).unwrap_or(c_uint::MAX)
}

fn encode(record: &Record) -> [u8; RECORD_BYTES] {
    let mut bytes = [0; RECORD_BYTES];
    put_u64(&mut bytes, record.seq);
    put_u32(&mut bytes[AT_AT..], record.at.unwrap_or(0));
    bytes[BODY_AT..CRC_AT].copy_from_slice(&record.body);
    let crc = crc32(&bytes[..CRC_AT]);
    put_u32(&mut bytes[CRC_AT..], crc);
    bytes
}

fn decode(bytes: &[u8; RECORD_BYTES]) -> Option<Record> {
    if get_u32(&bytes[CRC_AT..]) != crc32(&bytes[..CRC_AT]) {
        return None;
    }
    let seq = get_u64(bytes);
    if seq == 0 || seq == u64::MAX {
        return None;
    }
    let mut body = [0; BODY_BYTES];
    body.copy_from_slice(&bytes[BODY_AT..CRC_AT]);
    let at = get_u32(&bytes[AT_AT..]);
    Some(Record {
        seq,
        at: (at != 0).then_some(at),
        body,
    })
}
