//! Two-slot alternating device settings with a schema version, CRC-32 and a
//! commit marker, on the same slot I/O as the feed journal.
//!
//! The on-flash v1 record is 128 bytes: magic `PLFSET01`, version, record
//! size, generation, binding, bowl mode, flags (bit 0 lights, bit 1 sound),
//! volume, a zero byte, UTC offset in minutes, zero padding, CRC-32/ISO-HDLC
//! of bytes 0..120 at 120 and the `CMIT` commit marker at 124.
//!
//! Unlike the journal, losing the newest settings change is safe, so a torn
//! or invalid newer slot falls back to the older valid one. Storage with no
//! valid record, or holding a newer schema version, reads as defaults and is
//! never written until an explicit [`SettingsStore::reset`]: this firmware
//! does not overwrite data it cannot interpret.

use core::ffi::c_uint;

use wellfed_core::feed::progress::BowlMode;
use wellfed_core::store::slot::{
    Bound, COMMIT, COMMIT_AT, COMMIT_OFFSET, CRC_AT, LENGTH_AT, RECORD_BYTES, RECORD_LEN, SlotIo,
    TAIL_CHUNK, VERSION_AT, crc32, get_u16, get_u32, get_u64, put_u16, put_u32, put_u64,
};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFSET01";

/// Generations written before saves are refused: about 50 000 erases per
/// unit, half a typical NOR part's 100 000-cycle rating.
pub const WEAR_LIMIT: u64 = 100_000;

pub use crate::mailbox::MAX_VOLUME;
pub const MIN_UTC_OFFSET: i16 = -12 * 60;
pub const MAX_UTC_OFFSET: i16 = 14 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub bowl_mode: BowlMode,
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    pub utc_offset_minutes: i16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bowl_mode: BowlMode::Single,
            lights: true,
            sound: true,
            volume: 50,
            utc_offset_minutes: 0,
        }
    }
}

impl Settings {
    pub fn valid(&self) -> bool {
        self.volume <= MAX_VOLUME
            && (MIN_UTC_OFFSET..=MAX_UTC_OFFSET).contains(&self.utc_offset_minutes)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SettingsStatus {
    /// A valid record; the other slot is erased or holds the older one.
    #[default]
    Loaded = 0,
    /// Both slots erased: defaults, writable.
    Empty = 1,
    /// A valid record, with the other slot torn or invalid.
    Recovered = 2,
    /// No valid record: defaults, read-only until reset.
    Corrupt = 3,
    /// A newer schema version is present: defaults, read-only until reset.
    Future = 4,
    Io = 5,
    /// The wear limit is reached: saves are refused.
    Worn = 6,
    Unopened = 7,
}

impl SettingsStatus {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SlotState {
    /// Not read by the last open.
    #[default]
    Unread = 0,
    Erased = 1,
    Valid = 2,
    Future = 3,
    /// A prefix of our own record without its commit: a save cut short.
    Dirty = 4,
    /// Anything else, including a unit whose erase was cut short.
    Invalid = 5,
}

impl SlotState {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Erased,
    Valid(u64, Settings),
    Future,
    Dirty,
    Invalid,
}

impl Scan {
    fn state(self) -> SlotState {
        match self {
            Scan::Erased => SlotState::Erased,
            Scan::Valid(..) => SlotState::Valid,
            Scan::Future => SlotState::Future,
            Scan::Dirty => SlotState::Dirty,
            Scan::Invalid => SlotState::Invalid,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SettingsStore<I> {
    io: Bound<I>,
    binding: u64,
    pub settings: Settings,
    pub status: SettingsStatus,
    pub generation: u64,
    pub slot: c_uint,
    /// What the last open found in each slot.
    pub slots: [SlotState; 2],
    writable: bool,
}

impl<I> Default for SettingsStore<I> {
    fn default() -> Self {
        Self {
            io: Bound::NONE,
            binding: 0,
            settings: Settings::default(),
            status: SettingsStatus::Unopened,
            generation: 0,
            slot: 0,
            slots: [SlotState::Unread; 2],
            writable: false,
        }
    }
}

/// Programming lays bytes down in order, so a save cut short leaves a prefix
/// of the magic followed by erased bytes, or the whole magic. Anything else
/// is not evidence of our own write.
fn ours_torn(record: &[u8; RECORD_BYTES]) -> bool {
    let matched = MAGIC.iter().zip(record).take_while(|(m, r)| m == r).count();
    matched == MAGIC.len() || record[matched..].iter().all(|&b| b == u8::MAX)
}

impl<I: SlotIo> SettingsStore<I> {
    fn encode(&self, generation: u64, s: &Settings) -> [u8; RECORD_BYTES] {
        let mut p = [0u8; RECORD_BYTES];
        p[..8].copy_from_slice(&MAGIC);
        put_u16::<VERSION_AT, _>(&mut p, VERSION);
        put_u16::<LENGTH_AT, _>(&mut p, RECORD_LEN);
        put_u64::<12, _>(&mut p, generation);
        put_u64::<20, _>(&mut p, self.binding);

        p[28] = s.bowl_mode.byte();
        p[29] = u8::from(s.lights) | (u8::from(s.sound) << 1);
        p[30] = s.volume;
        put_u16::<32, _>(&mut p, s.utc_offset_minutes as u16);

        let crc = crc32(&p[..CRC_AT]);
        put_u32::<CRC_AT, _>(&mut p, crc);
        p[COMMIT_AT..].copy_from_slice(&COMMIT);
        p
    }

    fn decode(&self, p: &[u8; RECORD_BYTES]) -> Scan {
        if p[..8] != MAGIC
            || usize::from(get_u16::<LENGTH_AT, _>(p)) != RECORD_BYTES
            || get_u32::<CRC_AT, _>(p) != crc32(&p[..CRC_AT])
        {
            return Scan::Invalid;
        }

        let version = get_u16::<VERSION_AT, _>(p);
        if version > VERSION {
            return Scan::Future;
        }
        // Only v1 exists; an older version would be migrated here.
        if version != VERSION || get_u64::<20, _>(p) != self.binding {
            return Scan::Invalid;
        }

        let Some(bowl_mode) = BowlMode::from_byte(p[28]) else {
            return Scan::Invalid;
        };
        let s = Settings {
            bowl_mode,
            lights: p[29] & 1 != 0,
            sound: p[29] & 2 != 0,
            volume: p[30],
            utc_offset_minutes: get_u16::<32, _>(p) as i16,
        };
        let generation = get_u64::<12, _>(p);
        if p[29] & !3 != 0
            || p[31] != 0
            || p[34..CRC_AT].iter().any(|&b| b != 0)
            || !s.valid()
            || generation == 0
        {
            return Scan::Invalid;
        }

        Scan::Valid(generation, s)
    }

    /// The whole unit: one record, then erased bytes.
    fn scan(&self, slot: c_uint) -> Option<Scan> {
        let mut record = [0u8; RECORD_BYTES];
        if !self.io.read(slot, 0, &mut record) {
            return None;
        }

        let mut tail = [0u8; 64];
        let mut offset = u32::from(RECORD_LEN);
        let mut tail_erased = true;
        while offset < self.io.unit_bytes() {
            let chunk = (self.io.unit_bytes() - offset).min(TAIL_CHUNK);
            let size = chunk as usize;
            if !self.io.read(slot, offset, &mut tail[..size]) {
                return None;
            }
            tail_erased &= tail[..size].iter().all(|&b| b == u8::MAX);
            offset += chunk;
        }

        let erased = record.iter().all(|&b| b == u8::MAX);
        Some(match (erased, tail_erased) {
            (true, true) => Scan::Erased,
            (_, false) => Scan::Invalid,
            _ if record[COMMIT_AT..] != COMMIT && ours_torn(&record) => Scan::Dirty,
            _ if record[COMMIT_AT..] != COMMIT => Scan::Invalid,
            _ => self.decode(&record),
        })
    }

    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I, binding: u64) -> SettingsStatus {
        let unit = io.unit_bytes();
        *self = Self {
            io: Bound::new(io),
            binding,
            ..Self::default()
        };

        if (unit as usize) < RECORD_BYTES || !unit.is_multiple_of(4) || binding == 0 {
            return self.status;
        }

        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = SettingsStatus::Io;
            return self.status;
        };
        self.slots = [a.state(), b.state()];
        let pick = match (a, b) {
            (Scan::Valid(ga, sa), Scan::Valid(gb, sb)) if ga != gb => {
                Some(if ga > gb { (0, ga, sa) } else { (1, gb, sb) })
            }
            (Scan::Valid(g, s), _) => Some((0, g, s)),
            (_, Scan::Valid(g, s)) => Some((1, g, s)),
            _ => None,
        };

        let future = a == Scan::Future || b == Scan::Future;
        self.status = match pick {
            // A newer schema anywhere means this firmware must not write.
            _ if future => SettingsStatus::Future,
            Some(_) if matches!(a, Scan::Valid(..)) && matches!(b, Scan::Valid(..)) => {
                SettingsStatus::Loaded
            }
            Some(_) if a == Scan::Erased || b == Scan::Erased => SettingsStatus::Loaded,
            Some(_) => SettingsStatus::Recovered,
            None if a == Scan::Erased && b == Scan::Erased => SettingsStatus::Empty,
            // A torn first save never committed anything: still empty.
            None if matches!(
                (a, b),
                (Scan::Dirty, Scan::Erased) | (Scan::Erased, Scan::Dirty)
            ) =>
            {
                SettingsStatus::Empty
            }
            None => SettingsStatus::Corrupt,
        };

        if let Some((slot, generation, settings)) = pick.filter(|_| !future) {
            self.slot = slot;
            self.generation = generation;
            self.settings = settings;
        }
        self.writable = matches!(
            self.status,
            SettingsStatus::Loaded | SettingsStatus::Empty | SettingsStatus::Recovered
        );

        self.status
    }

    /// Write `settings` to the other slot: erase, program, verify, commit,
    /// verify. A failure keeps the previous settings in memory; the slot it
    /// touched may be torn, which the next open falls back from.
    pub fn save(&mut self, settings: &Settings) -> bool {
        if !self.writable || !settings.valid() {
            return false;
        }

        if self.generation >= WEAR_LIMIT {
            self.status = SettingsStatus::Worn;
            return false;
        }

        let generation = self.generation + 1;
        let slot = if self.generation == 0 {
            0
        } else {
            1 - self.slot
        };

        let bytes = self.encode(generation, settings);
        let mut checked = [0u8; RECORD_BYTES];
        let ok = self.io.erase(slot)
            && matches!(self.scan(slot), Some(Scan::Erased))
            && self.io.program(slot, 0, &bytes[..COMMIT_AT])
            && self.io.read(slot, 0, &mut checked)
            && bytes[..COMMIT_AT] == checked[..COMMIT_AT]
            && checked[COMMIT_AT..].iter().all(|&b| b == u8::MAX)
            && self.io.program(slot, COMMIT_OFFSET, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;

        if !ok {
            self.status = SettingsStatus::Io;
            return false;
        }

        self.slot = slot;
        self.generation = generation;
        self.settings = *settings;
        self.status = SettingsStatus::Loaded;
        true
    }

    /// Explicit reset (owner action): erase both units and return to writable
    /// defaults. Clears CORRUPT and FUTURE. Generation restarts at 0, so wear
    /// accounting must not depend on it across resets.
    pub fn reset(&mut self) -> bool {
        if self.binding == 0 {
            return false;
        }

        let ok = self.io.erase(0)
            && self.io.erase(1)
            && matches!(self.scan(0), Some(Scan::Erased))
            && matches!(self.scan(1), Some(Scan::Erased));

        self.settings = Settings::default();
        self.generation = 0;
        self.slot = 0;
        self.writable = ok;
        self.status = if ok {
            SettingsStatus::Empty
        } else {
            SettingsStatus::Io
        };
        ok
    }
}
