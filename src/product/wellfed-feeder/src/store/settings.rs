//! Two-slot alternating device settings, in the dual-slot store
//! (`wellfed_core::store::slot`) as the feed journal is.
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
//! does not overwrite data it cannot interpret. Only a save of its own cut
//! short beside an erased unit ([`SlotState::Dirty`]) still reads as empty.

use core::ffi::c_uint;

use wellfed_core::feed::progress::BowlMode;
use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Found, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u16, get_u64,
    newest, put_u16, put_u64, seal, target,
};

pub const VERSION: u16 = 1;
/// Settings read their whole units, as the journal does, so data another
/// writer left is never taken for empty settings and overwritten.
const LAYOUT: Layout = Layout {
    magic: *b"PLFSET01",
    version: VERSION,
    extent: Extent::Unit,
};
const GENERATION_AT: usize = 12;
const BINDING_AT: usize = 20;

/// Generations written before saves are refused: about 50 000 erases per
/// unit, half a typical NOR part's 100 000-cycle rating.
pub const WEAR_LIMIT: u64 = 100_000;

pub use wellfed_device_api::mailbox::MAX_VOLUME;
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

    /// The torn-write rule's state as settings report it: only a save of
    /// its own cut short is dirty.
    fn of<V>(found: &Found<V>) -> Self {
        match found {
            Found::Erased => Self::Erased,
            Found::Valid(_) => Self::Valid,
            Found::Future => Self::Future,
            Found::Partial => Self::Dirty,
            Found::Unmarked | Found::Torn | Found::Invalid => Self::Invalid,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SettingsStore<I> {
    units: DualSlot<I>,
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
            units: DualSlot::unbound(&LAYOUT),
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

impl<I: SlotIo> SettingsStore<I> {
    fn encode(&self, generation: u64, s: &Settings) -> [u8; RECORD_BYTES] {
        let mut p = LAYOUT.record();
        put_u64::<GENERATION_AT, _>(&mut p, generation);
        put_u64::<BINDING_AT, _>(&mut p, self.binding);

        p[28] = s.bowl_mode.byte();
        p[29] = u8::from(s.lights) | (u8::from(s.sound) << 1);
        p[30] = s.volume;
        put_u16::<32, _>(&mut p, s.utc_offset_minutes as u16);

        seal(&mut p);
        p
    }

    /// The generation and the settings of a record whose header and
    /// checksum hold. Only v1 exists; an older version would be migrated
    /// in the slot layer's version check.
    fn decode(&self, p: &[u8; RECORD_BYTES]) -> Option<(u64, Settings)> {
        if get_u64::<BINDING_AT, _>(p) != self.binding {
            return None;
        }

        let bowl_mode = BowlMode::from_byte(p[28])?;
        let s = Settings {
            bowl_mode,
            lights: p[29] & 1 != 0,
            sound: p[29] & 2 != 0,
            volume: p[30],
            utc_offset_minutes: get_u16::<32, _>(p) as i16,
        };
        let generation = get_u64::<GENERATION_AT, _>(p);
        if p[29] & !3 != 0
            || p[31] != 0
            || p[34..CRC_AT].iter().any(|&b| b != 0)
            || !s.valid()
            || generation == 0
        {
            return None;
        }

        Some((generation, s))
    }

    fn scan(&self, slot: c_uint) -> Option<Found<(u64, Settings)>> {
        self.units.scan(slot, |p| self.decode(p))
    }

    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I, binding: u64) -> SettingsStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
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
        self.slots = [SlotState::of(&a), SlotState::of(&b)];

        let both_valid = a.is_valid() && b.is_valid();
        let either_erased = a.is_erased() || b.is_erased();
        // A torn first save never committed anything: still empty.
        let first_cut = matches!(
            (a, b),
            (Found::Partial, Found::Erased) | (Found::Erased, Found::Partial)
        );
        self.status = match newest(a, b, |&(generation, _)| generation) {
            // A newer schema anywhere means this firmware must not write.
            Pick::Future => SettingsStatus::Future,
            Pick::Newest(slot, (generation, settings)) => {
                self.slot = slot;
                self.generation = generation;
                self.settings = settings;
                if both_valid || either_erased {
                    SettingsStatus::Loaded
                } else {
                    SettingsStatus::Recovered
                }
            }
            Pick::Nothing if (a.is_erased() && b.is_erased()) || first_cut => SettingsStatus::Empty,
            Pick::Nothing => SettingsStatus::Corrupt,
        };

        self.writable = matches!(
            self.status,
            SettingsStatus::Loaded | SettingsStatus::Empty | SettingsStatus::Recovered
        );

        self.status
    }

    /// Write `settings` to the other slot by the torn-write rule. A failure
    /// keeps the previous settings in memory; the slot it touched may be
    /// torn, which the next open falls back from.
    pub fn save(&mut self, settings: &Settings) -> bool {
        if !self.writable || !settings.valid() {
            return false;
        }

        if self.generation >= WEAR_LIMIT {
            self.status = SettingsStatus::Worn;
            return false;
        }

        let generation = self.generation + 1;
        let slot = target(self.generation, self.slot);
        if !self
            .units
            .write(slot, &self.encode(generation, settings), Prepare::Erase)
        {
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

        let ok = self.units.clear();

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
