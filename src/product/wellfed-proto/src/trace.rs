//! One feed's trace body: what the feeding owner saw at each poll of one
//! drive, packed so a whole feed fits in under a kilobyte of the feeder's
//! RAM and in one storage slot, and travels in two frames
//! ([`api::TracePage`](crate::api::TracePage)).
//!
//! The body is a run of entries, each timed in milliseconds from the
//! drive's start and coded against the entry before it:
//!
//! | First byte | Then | Entry |
//! | --- | --- | --- |
//! | `0ddddddd` | zigzag varint: the current's change | [`Entry::Current`] |
//! | `1000dddd` | varint: `edges << 1 \| high` | [`Entry::Mark`] |
//! | `1001dddd` | one byte: bit 0 outlet clear, bit 1 food seen | [`Entry::Beams`] |
//! | `1010dddd` | zigzag varint: the battery's change | [`Entry::Battery`] |
//! | `1011dddd` | one byte: the motor command | [`Entry::Motor`] |
//!
//! `d` is the time since the entry before; its largest value (127 for a
//! current reading, 15 for the others) says a varint of the rest follows.
//! The first current and battery readings are changes from 0. Varints are
//! LEB128; a zigzag varint maps 0, -1, 1, -2 … to 0, 1, 2, 3 … first.
//! Kinds `1100` to `1111` are reserved: a reader refuses them.
//!
//! **A full body halves its current readings** ([`Writer::halve`]): the
//! second of each pair goes, and its time and change fold into the entry
//! after it. Neither a time nor a change takes more bytes coded
//! as a sum than as its parts, so the rewrite never overtakes what it reads
//! and runs in place. Marks, beams, battery and motor entries are kept;
//! once only those are left and the body is still full, it ends there,
//! truncated. The feed's summary keeps the true peak, mean and counts
//! either way.

/// The longest body: with a 96-byte header and its checksum, one in a
/// kilobyte storage slot.
pub const BODY_MAX: usize = 864;
/// The most body one [`api::TracePage`](crate::api::TracePage) carries:
/// half the longest, so a whole body is two pages.
pub const PAGE_MAX: usize = BODY_MAX / 2;

/// A current reading's own time field: 0..=126, then a varint.
const CURRENT_TIME: u32 = 0x7f;
/// Every other entry's time field: 0..=14, then a varint.
const EVENT_TIME: u32 = 0x0f;
const EVENT: u8 = 0x80;
const MARK: u8 = 0;
const BEAMS: u8 = 1;
const BATTERY: u8 = 2;
const MOTOR: u8 = 3;
/// The longest entry: its first byte, a time varint and a payload varint.
const ENTRY_MAX: usize = 1 + 5 + 5;

/// The motor command as the controller gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motor {
    Stop,
    /// `MotorCommand::Drive0`: a single bowl's feeding direction.
    Drive0,
    Drive1,
}

impl Motor {
    const fn code(self) -> u8 {
        match self {
            Self::Stop => 0,
            Self::Drive0 => 1,
            Self::Drive1 => 2,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Stop),
            1 => Some(Self::Drive0),
            2 => Some(Self::Drive1),
            _ => None,
        }
    }
}

/// One entry, with its time in milliseconds from the drive's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The motor current at one poll, in the summary's unit.
    Current { at_ms: u32, value: u32 },
    /// Feedback edges seen since the last mark entry: `at_ms` is the last
    /// one's time, and `high` the level after it.
    Mark { at_ms: u32, edges: u32, high: bool },
    /// The beams as one poll read them, when they changed: the outlet beam
    /// clear, and food in the food beam.
    Beams {
        at_ms: u32,
        outlet_clear: bool,
        food: bool,
    },
    /// The battery pack, in millivolts, when it moved.
    Battery { at_ms: u32, mv: u32 },
    /// The motor command, when it changed.
    Motor { at_ms: u32, motor: Motor },
}

impl Entry {
    pub const fn at_ms(&self) -> u32 {
        match *self {
            Self::Current { at_ms, .. }
            | Self::Mark { at_ms, .. }
            | Self::Beams { at_ms, .. }
            | Self::Battery { at_ms, .. }
            | Self::Motor { at_ms, .. } => at_ms,
        }
    }

    const fn with_at(self, at_ms: u32) -> Self {
        match self {
            Self::Current { value, .. } => Self::Current { at_ms, value },
            Self::Mark { edges, high, .. } => Self::Mark { at_ms, edges, high },
            Self::Beams {
                outlet_clear, food, ..
            } => Self::Beams {
                at_ms,
                outlet_clear,
                food,
            },
            Self::Battery { mv, .. } => Self::Battery { at_ms, mv },
            Self::Motor { motor, .. } => Self::Motor { at_ms, motor },
        }
    }
}

/// What the body does not hold: bytes that are not entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the trace body is malformed")]
pub struct Malformed;

/// What an entry is coded against: the entry before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Base {
    at_ms: u32,
    current: u32,
    battery: u32,
}

/// Whether an entry went in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    Kept,
    /// No room: [`Writer::halve`] may make some.
    Full,
}

/// Builds one body in place.
#[derive(Clone, Debug)]
pub struct Writer<const N: usize = BODY_MAX> {
    body: [u8; N],
    len: usize,
    base: Base,
    currents: u32,
}

impl<const N: usize> Default for Writer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Writer<N> {
    pub const fn new() -> Self {
        Self {
            body: [0; N],
            len: 0,
            base: Base {
                at_ms: 0,
                current: 0,
                battery: 0,
            },
            currents: 0,
        }
    }

    /// Empty the body in place, for the next trace.
    pub fn clear(&mut self) {
        self.len = 0;
        self.base = Base::default();
        self.currents = 0;
    }

    /// The body so far.
    pub fn bytes(&self) -> &[u8] {
        self.body.get(..self.len).unwrap_or(&[])
    }

    /// Current readings the body holds.
    pub const fn currents(&self) -> u32 {
        self.currents
    }

    /// Append `entry`. One timed before the last entry is held at the last
    /// entry's time: an edge the interrupt stamped just before the poll
    /// that read it.
    pub fn push(&mut self, entry: Entry) -> Pushed {
        let entry = entry.with_at(entry.at_ms().max(self.base.at_ms));
        let mut scratch = [0; ENTRY_MAX];
        let mut base = self.base;
        let used = encode(entry, &mut base, &mut scratch);
        let Some(room) = self.body.get_mut(self.len..self.len + used) else {
            return Pushed::Full;
        };
        room.copy_from_slice(&scratch[..used]);
        self.len += used;
        self.base = base;
        if matches!(entry, Entry::Current { .. }) {
            self.currents += 1;
        }
        Pushed::Kept
    }

    /// Drop every other current reading, the second of each pair, so the
    /// first reading (a drive's start-up surge) stays; false when that
    /// freed nothing (one reading or none left). The body is rewritten in
    /// place: no entry takes more bytes than those it replaces, which the
    /// loop checks as it goes and, if it ever found otherwise, ends the
    /// body there.
    pub fn halve(&mut self) -> bool {
        let before = self.len;
        let mut read = 0;
        let mut write = 0;
        let mut decoded = Base::default();
        let mut encoded = Base::default();
        let mut nth = 0u32;
        let mut kept = 0u32;
        while read < self.len {
            let Ok((entry, used)) = decode(&self.body[read..self.len], &mut decoded) else {
                break;
            };
            read += used;
            let current = matches!(entry, Entry::Current { .. });
            if current {
                nth += 1;
                if nth.is_multiple_of(2) {
                    continue;
                }
            }
            let mut scratch = [0; ENTRY_MAX];
            let mut base = encoded;
            let n = encode(entry, &mut base, &mut scratch);
            if write + n > read {
                break;
            }
            self.body[write..write + n].copy_from_slice(&scratch[..n]);
            write += n;
            encoded = base;
            kept += u32::from(current);
        }

        self.len = write;
        self.base = encoded;
        self.currents = kept;
        self.len < before
    }
}

/// The entries of a body, in order; a malformed one ends it.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    rest: &'a [u8],
    base: Base,
}

impl<'a> Reader<'a> {
    pub const fn new(body: &'a [u8]) -> Self {
        Self {
            rest: body,
            base: Base {
                at_ms: 0,
                current: 0,
                battery: 0,
            },
        }
    }
}

impl Iterator for Reader<'_> {
    type Item = Result<Entry, Malformed>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        match decode(self.rest, &mut self.base) {
            Ok((entry, used)) => {
                self.rest = self.rest.get(used..).unwrap_or(&[]);
                Some(Ok(entry))
            }
            Err(error) => {
                self.rest = &[];
                Some(Err(error))
            }
        }
    }
}

fn encode(entry: Entry, base: &mut Base, out: &mut [u8; ENTRY_MAX]) -> usize {
    let dt = entry.at_ms().saturating_sub(base.at_ms);
    base.at_ms = entry.at_ms();
    let (first, limit) = match entry {
        Entry::Current { .. } => (0, CURRENT_TIME),
        Entry::Mark { .. } => (EVENT | (MARK << 4), EVENT_TIME),
        Entry::Beams { .. } => (EVENT | (BEAMS << 4), EVENT_TIME),
        Entry::Battery { .. } => (EVENT | (BATTERY << 4), EVENT_TIME),
        Entry::Motor { .. } => (EVENT | (MOTOR << 4), EVENT_TIME),
    };

    let mut at = 1;
    if dt < limit {
        out[0] = first | low7(dt);
    } else {
        out[0] = first | low7(limit);
        at += varint(u64::from(dt - limit), &mut out[at..]);
    }
    at += match entry {
        Entry::Current { value, .. } => {
            let change = i64::from(value) - i64::from(base.current);
            base.current = value;
            varint(zigzag(change), &mut out[at..])
        }
        Entry::Battery { mv, .. } => {
            let change = i64::from(mv) - i64::from(base.battery);
            base.battery = mv;
            varint(zigzag(change), &mut out[at..])
        }
        Entry::Mark { edges, high, .. } => {
            varint((u64::from(edges) << 1) | u64::from(high), &mut out[at..])
        }
        Entry::Beams {
            outlet_clear, food, ..
        } => {
            out[at] = u8::from(outlet_clear) | (u8::from(food) << 1);
            1
        }
        Entry::Motor { motor, .. } => {
            out[at] = motor.code();
            1
        }
    };
    at
}

fn decode(bytes: &[u8], base: &mut Base) -> Result<(Entry, usize), Malformed> {
    let (&first, _) = bytes.split_first().ok_or(Malformed)?;
    let mut at = 1;
    let (kind, field, limit) = if first & EVENT == 0 {
        (None, u32::from(first), CURRENT_TIME)
    } else {
        (
            Some((first >> 4) & 0x07),
            u32::from(first & 0x0f),
            EVENT_TIME,
        )
    };
    let dt = if field == limit {
        let (rest, used) = read_varint(&bytes[at..])?;
        at += used;
        u32::try_from(rest)
            .ok()
            .and_then(|rest| rest.checked_add(limit))
            .ok_or(Malformed)?
    } else {
        field
    };
    let at_ms = base.at_ms.checked_add(dt).ok_or(Malformed)?;

    let payload = &bytes[at..];
    let (entry, used) = match kind {
        None => {
            let (change, used) = read_varint(payload)?;
            let value = apply(base.current, unzigzag(change))?;
            base.current = value;
            (Entry::Current { at_ms, value }, used)
        }
        Some(BATTERY) => {
            let (change, used) = read_varint(payload)?;
            let mv = apply(base.battery, unzigzag(change))?;
            base.battery = mv;
            (Entry::Battery { at_ms, mv }, used)
        }
        Some(MARK) => {
            let (packed, used) = read_varint(payload)?;
            let edges = u32::try_from(packed >> 1).map_err(|_| Malformed)?;
            let high = packed & 1 != 0;
            (Entry::Mark { at_ms, edges, high }, used)
        }
        Some(BEAMS) => {
            let &flags = payload.first().ok_or(Malformed)?;
            if flags & !0x03 != 0 {
                return Err(Malformed);
            }
            let entry = Entry::Beams {
                at_ms,
                outlet_clear: flags & 1 != 0,
                food: flags & 2 != 0,
            };
            (entry, 1)
        }
        Some(MOTOR) => {
            let &code = payload.first().ok_or(Malformed)?;
            let motor = Motor::from_code(code).ok_or(Malformed)?;
            (Entry::Motor { at_ms, motor }, 1)
        }
        Some(_) => return Err(Malformed),
    };
    base.at_ms = at_ms;
    Ok((entry, at + used))
}

/// The low seven bits, which every caller has already bounded.
fn low7(value: impl Into<u64>) -> u8 {
    u8::try_from(value.into() & 0x7f).unwrap_or(0)
}

fn apply(base: u32, change: i64) -> Result<u32, Malformed> {
    u32::try_from(i64::from(base) + change).map_err(|_| Malformed)
}

const fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)).cast_unsigned()
}

const fn unzigzag(value: u64) -> i64 {
    (value >> 1).cast_signed() ^ -((value & 1).cast_signed())
}

/// LEB128 into `out`; the bytes used. A u64 below 2^35 (every value here)
/// takes at most 5.
fn varint(mut value: u64, out: &mut [u8]) -> usize {
    let mut at = 0;
    loop {
        let byte = low7(value);
        value >>= 7;
        let Some(slot) = out.get_mut(at) else {
            return at;
        };
        at += 1;
        if value == 0 {
            *slot = byte;
            return at;
        }
        *slot = byte | 0x80;
    }
}

/// A LEB128 value of at most 5 bytes, and the bytes it took.
fn read_varint(bytes: &[u8]) -> Result<(u64, usize), Malformed> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().take(5).enumerate() {
        value |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(Malformed)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::*;

    fn entries(body: &[u8]) -> impl Iterator<Item = Entry> + '_ {
        Reader::new(body).map(|entry| entry.unwrap())
    }

    #[test]
    fn every_kind_reads_back() {
        let written = [
            Entry::Motor {
                at_ms: 0,
                motor: Motor::Drive0,
            },
            Entry::Battery {
                at_ms: 0,
                mv: 4_810,
            },
            Entry::Current {
                at_ms: 17,
                value: 333,
            },
            Entry::Current {
                at_ms: 47,
                value: 61,
            },
            Entry::Beams {
                at_ms: 60,
                outlet_clear: false,
                food: true,
            },
            Entry::Mark {
                at_ms: 7_840,
                edges: 1,
                high: false,
            },
            Entry::Battery {
                at_ms: 7_900,
                mv: 4_700,
            },
            Entry::Current {
                at_ms: 9_000_000,
                value: u32::MAX,
            },
            Entry::Current {
                at_ms: 9_000_001,
                value: 0,
            },
            Entry::Mark {
                at_ms: u32::MAX,
                edges: 3,
                high: true,
            },
            Entry::Motor {
                at_ms: u32::MAX,
                motor: Motor::Stop,
            },
        ];
        let mut writer = Writer::<128>::new();
        for entry in written {
            assert_eq!(writer.push(entry), Pushed::Kept);
        }
        assert!(entries(writer.bytes()).eq(written));
        assert_eq!(writer.currents(), 4);
    }

    #[test]
    fn a_steady_reading_takes_two_bytes() {
        let mut writer = Writer::<64>::new();
        for poll in 0..10 {
            writer.push(Entry::Current {
                at_ms: 30 * poll,
                value: 60 + poll % 3,
            });
        }
        assert_eq!(writer.bytes().len(), 20);
    }

    #[test]
    fn an_entry_before_the_last_is_held_at_its_time() {
        let mut writer = Writer::<32>::new();
        writer.push(Entry::Current {
            at_ms: 100,
            value: 5,
        });
        writer.push(Entry::Mark {
            at_ms: 90,
            edges: 1,
            high: true,
        });
        let marks: Vec<_> = entries(writer.bytes()).collect();
        assert_eq!(
            marks[1],
            Entry::Mark {
                at_ms: 100,
                edges: 1,
                high: true
            }
        );
    }

    #[test]
    fn halving_keeps_every_other_reading_and_every_event() {
        let mut writer = Writer::<256>::new();
        let mut written = Vec::new();
        for poll in 0..40u32 {
            let entry = Entry::Current {
                at_ms: 30 * poll,
                value: 60 + poll,
            };
            written.push(entry);
            writer.push(entry);
            if poll % 9 == 4 {
                let mark = Entry::Mark {
                    at_ms: 30 * poll + 1,
                    edges: 1,
                    high: poll % 2 == 0,
                };
                written.push(mark);
                writer.push(mark);
            }
        }
        let before = writer.bytes().len();

        assert!(writer.halve());
        let mut nth = 0;
        let expected: Vec<Entry> = written
            .into_iter()
            .filter(|entry| match entry {
                Entry::Current { .. } => {
                    nth += 1;
                    nth % 2 == 1
                }
                _ => true,
            })
            .collect();
        assert!(entries(writer.bytes()).eq(expected.iter().copied()));
        assert!(writer.bytes().len() < before);
        assert_eq!(writer.currents(), 20);

        // Appends after a halving code against what is left.
        writer.push(Entry::Current {
            at_ms: 5_000,
            value: 7,
        });
        assert_eq!(
            entries(writer.bytes()).last(),
            Some(Entry::Current {
                at_ms: 5_000,
                value: 7
            })
        );
    }

    #[test]
    fn a_body_of_events_alone_cannot_halve() {
        let mut writer = Writer::<16>::new();
        writer.push(Entry::Mark {
            at_ms: 1,
            edges: 1,
            high: false,
        });
        assert!(!writer.halve());
        assert_eq!(writer.bytes().len(), 2);
    }

    #[test]
    fn a_full_body_refuses_the_entry_whole() {
        let mut writer = Writer::<4>::new();
        assert_eq!(
            writer.push(Entry::Current { at_ms: 1, value: 1 }),
            Pushed::Kept
        );
        assert_eq!(
            writer.push(Entry::Current { at_ms: 2, value: 2 }),
            Pushed::Kept
        );
        assert_eq!(
            writer.push(Entry::Current {
                at_ms: 3,
                value: 300
            }),
            Pushed::Full
        );
        assert_eq!(entries(writer.bytes()).count(), 2);
    }

    #[test]
    fn malformed_bodies_are_refused() {
        // A reserved kind, a varint that never ends, a beam byte with
        // unknown bits, a motor code past the three, and a time past u32.
        for body in [
            &[0xc0][..],
            &[0x7f, 0x80, 0x80, 0x80, 0x80, 0x80],
            &[0x90, 0x04],
            &[0xb0, 0x03],
            &[0x7f, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x00],
            &[0x01],
        ] {
            assert_eq!(
                Reader::new(body).last(),
                Some(Err(Malformed)),
                "{body:02x?}"
            );
        }
    }
}
