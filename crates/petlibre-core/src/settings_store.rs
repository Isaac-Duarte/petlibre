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

use crate::feed_progress::BowlMode;
use crate::storage::{COMMIT, JournalIo, RECORD_BYTES, crc32, get, put};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFSET01";

/// Generations written before saves are refused: about 50 000 erases per
/// unit, half a typical NOR part's 100 000-cycle rating.
pub const WEAR_LIMIT: u64 = 100_000;

pub const MAX_VOLUME: u8 = 100;
pub const MIN_UTC_OFFSET: i16 = -12 * 60;
pub const MAX_UTC_OFFSET: i16 = 14 * 60;

#[repr(C)]
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
            bowl_mode: BowlMode::SINGLE,
            lights: true,
            sound: true,
            volume: 50,
            utc_offset_minutes: 0,
        }
    }
}

impl Settings {
    pub fn valid(&self) -> bool {
        self.bowl_mode.valid()
            && self.volume <= MAX_VOLUME
            && (MIN_UTC_OFFSET..=MAX_UTC_OFFSET).contains(&self.utc_offset_minutes)
    }
}

c_enum!(SettingsStatus {
    /// A valid record; the other slot is erased or holds the older one.
    LOADED = 0,
    /// Both slots erased: defaults, writable.
    EMPTY = 1,
    /// A valid record, with the other slot torn or invalid.
    RECOVERED = 2,
    /// No valid record: defaults, read-only until reset.
    CORRUPT = 3,
    /// A newer schema version is present: defaults, read-only until reset.
    FUTURE = 4,
    IO = 5,
    /// The wear limit is reached: saves are refused.
    WORN = 6,
    UNOPENED = 7,
});

c_enum!(SlotState {
    /// Not read by the last open.
    UNREAD = 0,
    ERASED = 1,
    VALID = 2,
    FUTURE = 3,
    /// A prefix of our own record without its commit: a save cut short.
    DIRTY = 4,
    /// Anything else, including a unit whose erase was cut short.
    INVALID = 5,
});

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
            Scan::Erased => SlotState::ERASED,
            Scan::Valid(..) => SlotState::VALID,
            Scan::Future => SlotState::FUTURE,
            Scan::Dirty => SlotState::DIRTY,
            Scan::Invalid => SlotState::INVALID,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SettingsStore {
    io: JournalIo,
    binding: u64,
    pub settings: Settings,
    pub status: SettingsStatus,
    pub generation: u64,
    pub slot: c_uint,
    /// What the last open found in each slot.
    pub slots: [SlotState; 2],
    writable: bool,
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self {
            io: JournalIo::default(),
            binding: 0,
            settings: Settings::default(),
            status: SettingsStatus::UNOPENED,
            generation: 0,
            slot: 0,
            slots: [SlotState::UNREAD; 2],
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

impl SettingsStore {
    fn encode(&self, generation: u64, s: &Settings) -> [u8; RECORD_BYTES] {
        let mut p = [0u8; RECORD_BYTES];
        p[..8].copy_from_slice(&MAGIC);
        put(&mut p[8..], u64::from(VERSION), 2);
        put(&mut p[10..], RECORD_BYTES as u64, 2);
        put(&mut p[12..], generation, 8);
        put(&mut p[20..], self.binding, 8);

        p[28] = s.bowl_mode.0 as u8;
        p[29] = u8::from(s.lights) | (u8::from(s.sound) << 1);
        p[30] = s.volume;
        put(&mut p[32..], s.utc_offset_minutes as u16 as u64, 2);

        let crc = crc32(&p[..120]);
        put(&mut p[120..], u64::from(crc), 4);
        p[124..].copy_from_slice(&COMMIT);
        p
    }

    fn decode(&self, p: &[u8; RECORD_BYTES]) -> Scan {
        if p[..8] != MAGIC
            || get(&p[10..], 2) != RECORD_BYTES as u64
            || get(&p[120..], 4) != u64::from(crc32(&p[..120]))
        {
            return Scan::Invalid;
        }

        let version = get(&p[8..], 2) as u16;
        if version > VERSION {
            return Scan::Future;
        }
        // Only v1 exists; an older version would be migrated here.
        if version != VERSION || get(&p[20..], 8) != self.binding {
            return Scan::Invalid;
        }

        let s = Settings {
            bowl_mode: BowlMode(u32::from(p[28])),
            lights: p[29] & 1 != 0,
            sound: p[29] & 2 != 0,
            volume: p[30],
            utc_offset_minutes: get(&p[32..], 2) as u16 as i16,
        };
        let generation = get(&p[12..], 8);
        if p[29] & !3 != 0
            || p[31] != 0
            || p[34..120].iter().any(|&b| b != 0)
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
        let mut offset = RECORD_BYTES as u32;
        let mut tail_erased = true;
        while offset < self.io.unit_bytes {
            let size = ((self.io.unit_bytes - offset) as usize).min(tail.len());
            if !self.io.read(slot, offset, &mut tail[..size]) {
                return None;
            }
            tail_erased &= tail[..size].iter().all(|&b| b == u8::MAX);
            offset += size as u32;
        }

        let erased = record.iter().all(|&b| b == u8::MAX);
        Some(match (erased, tail_erased) {
            (true, true) => Scan::Erased,
            (_, false) => Scan::Invalid,
            _ if record[124..] != COMMIT && ours_torn(&record) => Scan::Dirty,
            _ if record[124..] != COMMIT => Scan::Invalid,
            _ => self.decode(&record),
        })
    }

    /// Bind to two exclusive erase units and load the newest valid record.
    ///
    /// # Safety
    /// The I/O callbacks must be sound for `io.context` while the store is used.
    #[allow(unsafe_code)]
    pub unsafe fn open(&mut self, io: JournalIo, binding: u64) -> SettingsStatus {
        *self = Self {
            io,
            binding,
            ..Self::default()
        };

        if io.read.is_none()
            || io.program.is_none()
            || io.erase.is_none()
            || (io.unit_bytes as usize) < RECORD_BYTES
            || !io.unit_bytes.is_multiple_of(4)
            || binding == 0
        {
            return self.status;
        }

        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = SettingsStatus::IO;
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
            _ if future => SettingsStatus::FUTURE,
            Some(_) if matches!(a, Scan::Valid(..)) && matches!(b, Scan::Valid(..)) => {
                SettingsStatus::LOADED
            }
            Some(_) if a == Scan::Erased || b == Scan::Erased => SettingsStatus::LOADED,
            Some(_) => SettingsStatus::RECOVERED,
            None if a == Scan::Erased && b == Scan::Erased => SettingsStatus::EMPTY,
            // A torn first save never committed anything: still empty.
            None if matches!(
                (a, b),
                (Scan::Dirty, Scan::Erased) | (Scan::Erased, Scan::Dirty)
            ) =>
            {
                SettingsStatus::EMPTY
            }
            None => SettingsStatus::CORRUPT,
        };

        if let Some((slot, generation, settings)) = pick.filter(|_| !future) {
            self.slot = slot;
            self.generation = generation;
            self.settings = settings;
        }
        self.writable = matches!(
            self.status,
            SettingsStatus::LOADED | SettingsStatus::EMPTY | SettingsStatus::RECOVERED
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
            self.status = SettingsStatus::WORN;
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
            && self.io.program(slot, 0, &bytes[..124])
            && self.io.read(slot, 0, &mut checked)
            && bytes[..124] == checked[..124]
            && checked[124..].iter().all(|&b| b == u8::MAX)
            && self.io.program(slot, 124, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;

        if !ok {
            self.status = SettingsStatus::IO;
            return false;
        }

        self.slot = slot;
        self.generation = generation;
        self.settings = *settings;
        self.status = SettingsStatus::LOADED;
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
            SettingsStatus::EMPTY
        } else {
            SettingsStatus::IO
        };
        ok
    }
}
