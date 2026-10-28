//! Two-slot alternating device settings, in the dual-slot store
//! (`wellfed_core::store::slot`) as the feed journal is.
//!
//! The on-flash v4 record is 128 bytes: magic `PLFSET01`, version, record
//! size, generation, binding, bowl mode, flags (bit 0 lights, bit 1 sound),
//! volume, the meal reminder's lead in minutes (0 off), UTC offset in
//! minutes, the lights and sound windows (a kind byte each at 34 and 35, 0
//! always and 1 between, then each window's start and end minute as u16 at
//! 36..40 and 40..44), the desiccant reminder (its period in days at 44, 1
//! at 45 when a replacement is recorded, two zero bytes, and the
//! replacement's local calendar day as an i32 at 48..52, zero when none),
//! zero padding, CRC-32/ISO-HDLC of bytes 0..120 at 120 and the `CMIT`
//! commit marker at 124. A record in an older version (v1 without the
//! windows, v2 without the desiccant reminder, v3 without the meal
//! reminder) is never migrated: it counts as erased, so the store opens on
//! defaults and the next save replaces it.
//!
//! Unlike the journal, losing the newest settings change is safe, so a torn
//! or invalid newer slot falls back to the older valid one. Storage with no
//! valid record, or holding a newer schema version, reads as defaults and is
//! never written until an explicit [`SettingsStore::reset`]: this firmware
//! does not overwrite data it cannot interpret. Only a save of its own cut
//! short beside an erased unit ([`SlotState::Dirty`]) still reads as empty.

use core::ffi::c_uint;

use wellfed_core::desiccant;
use wellfed_core::feed::progress::BowlMode;
use wellfed_core::settings::{MealReminder, SettingsFields, SettingsView, Window};
use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Found, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u16, get_u32,
    get_u64, newest, put_u16, put_u32, put_u64, seal, target,
};

pub const VERSION: u16 = 4;
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
    pub lights_window: Window,
    pub sound_window: Window,
    /// The desiccant reminder's period in days.
    pub desiccant_days: u8,
    /// The local calendar day of the last desiccant replacement.
    pub desiccant_reset: Option<i32>,
    pub meal_reminder: MealReminder,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bowl_mode: BowlMode::Single,
            lights: true,
            sound: true,
            volume: 50,
            utc_offset_minutes: 0,
            lights_window: Window::Always,
            sound_window: Window::Always,
            desiccant_days: desiccant::PERIOD_DEFAULT,
            desiccant_reset: None,
            meal_reminder: MealReminder::Off,
        }
    }
}

impl Settings {
    pub fn valid(&self) -> bool {
        self.volume <= MAX_VOLUME
            && (MIN_UTC_OFFSET..=MAX_UTC_OFFSET).contains(&self.utc_offset_minutes)
            && self.lights_window.valid()
            && self.sound_window.valid()
            && desiccant::period_valid(self.desiccant_days)
            && self.meal_reminder.valid()
    }

    /// These settings with a request's fields applied; the writer's merge,
    /// shared with the host simulator so the two cannot drift.
    pub fn changed(self, fields: SettingsFields) -> Self {
        Self {
            lights: fields.lights.unwrap_or(self.lights),
            sound: fields.sound.unwrap_or(self.sound),
            volume: fields.volume.unwrap_or(self.volume),
            lights_window: fields.lights_window.unwrap_or(self.lights_window),
            sound_window: fields.sound_window.unwrap_or(self.sound_window),
            desiccant_days: fields.desiccant_days.unwrap_or(self.desiccant_days),
            desiccant_reset: fields.desiccant_reset.or(self.desiccant_reset),
            meal_reminder: fields.meal_reminder.unwrap_or(self.meal_reminder),
            ..self
        }
    }

    /// The view the writer publishes for these settings.
    pub fn view(&self, generation: u64, writable: bool) -> SettingsView {
        SettingsView {
            lights: self.lights,
            sound: self.sound,
            volume: self.volume,
            lights_window: self.lights_window,
            sound_window: self.sound_window,
            desiccant_days: self.desiccant_days,
            desiccant_reset: self.desiccant_reset,
            meal_reminder: self.meal_reminder,
            // Reported as 32 bits; saturate rather than wrap to an old value.
            generation: u32::try_from(generation).unwrap_or(u32::MAX),
            writable,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SettingsStatus {
    /// A valid record; the other slot is erased or holds the older one.
    #[default]
    Loaded = 0,
    /// Each slot erased or holding an older format version: defaults,
    /// writable.
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
    /// A record in an older format version of this store, superseded: no
    /// build migrates it, so it counts as erased.
    Older = 6,
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
            Found::Older => Self::Older,
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
        p[31] = s.meal_reminder.minutes().unwrap_or(0);
        put_u16::<32, _>(&mut p, s.utc_offset_minutes as u16);
        let (kind, start, end) = window_fields(s.lights_window);
        p[34] = kind;
        put_u16::<36, _>(&mut p, start);
        put_u16::<38, _>(&mut p, end);
        let (kind, start, end) = window_fields(s.sound_window);
        p[35] = kind;
        put_u16::<40, _>(&mut p, start);
        put_u16::<42, _>(&mut p, end);
        p[44] = s.desiccant_days;
        p[45] = u8::from(s.desiccant_reset.is_some());
        put_u32::<48, _>(&mut p, s.desiccant_reset.unwrap_or(0).cast_unsigned());

        seal(&mut p);
        p
    }

    /// The generation and the settings of a record whose header and
    /// checksum hold.
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
            lights_window: window(p[34], get_u16::<36, _>(p), get_u16::<38, _>(p))?,
            sound_window: window(p[35], get_u16::<40, _>(p), get_u16::<42, _>(p))?,
            desiccant_days: p[44],
            desiccant_reset: match p[45] {
                0 if get_u32::<48, _>(p) == 0 => None,
                1 => Some(get_u32::<48, _>(p).cast_signed()),
                _ => return None,
            },
            meal_reminder: match p[31] {
                0 => MealReminder::Off,
                minutes => MealReminder::Before { minutes },
            },
        };
        let generation = get_u64::<GENERATION_AT, _>(p);
        if p[29] & !3 != 0
            || p[46..48].iter().any(|&b| b != 0)
            || p[52..CRC_AT].iter().any(|&b| b != 0)
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

        // A record in an older version of this store is superseded, never
        // migrated: it counts as an erased unit, so a format change starts
        // from writable defaults rather than locking the store.
        let blank = |found: &Found<_>| found.is_erased() || matches!(found, Found::Older);
        let both_valid = a.is_valid() && b.is_valid();
        let either_erased = blank(&a) || blank(&b);
        // A torn first save never committed anything: still empty.
        let first_cut = (matches!(a, Found::Partial) && blank(&b))
            || (blank(&a) && matches!(b, Found::Partial));
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
            Pick::Nothing if (blank(&a) && blank(&b)) || first_cut => SettingsStatus::Empty,
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

/// A window's kind byte and its bounds, as the record keeps them (zero for
/// always).
fn window_fields(window: Window) -> (u8, u16, u16) {
    match window {
        Window::Always => (0, 0, 0),
        Window::Between { start, end } => (1, start, end),
    }
}

/// The window a record's kind byte and bounds name; `None` for an unknown
/// kind, or an always window with bounds.
fn window(kind: u8, start: u16, end: u16) -> Option<Window> {
    match kind {
        0 if start == 0 && end == 0 => Some(Window::Always),
        1 => Some(Window::Between { start, end }),
        _ => None,
    }
}
