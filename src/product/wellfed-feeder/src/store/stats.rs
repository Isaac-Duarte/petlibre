//! The lifetime statistics on NOR: what the feeder counted over every
//! boot, its boots by reset cause and its erases by storage sector.
//!
//! A 288-byte record in the dual-slot store (`wellfed_core::store::slot`),
//! little-endian: magic `PLFSTA01`, version, record size, generation (u64
//! at 12), the ten counts of `Counts` in wire order (u32 each at 20..60:
//! uptime, feeds, feeds not done, portions, presses, sessions, reconnects,
//! DHCP renewals, outages, seconds on battery), the five reset causes
//! (60..80: power-on, watchdog, software, brownout, other), bank 1's erases
//! (80), each storage sector's erases (84..276, sector 0 first), the early
//! saves' tokens in use (276, at most `EARLY_SAVES`), flags
//! (277: bit 0, the early budget was spent), zero padding, CRC-32/ISO-HDLC
//! of bytes 0..280 at 280 and the `CMIT` commit marker at 284.
//!
//! The erase list covers `TOTAL_SECTORS`, the region as it is now: growing
//! the region again changes the record's layout, so it takes a new
//! version (and this build reads the old one as invalid, as no migration
//! exists).
//!
//! **Early saves** are the one each boot makes as it starts, so its reset
//! is counted even if it ends at once, and one at an intentional restart
//! within `RESTART_SETTLED_MS` of boot (a later restart's save is
//! `SaveKind::Restart`, outside the budget).
//! Each takes one of `EARLY_SAVES` tokens and each settled save (the
//! hourly one) gives one back, so early saves never outnumber settled ones
//! by more than `EARLY_SAVES`, which bounds the store's wear under any
//! crash pattern. The save that takes the last token sets a sticky flag:
//! boots after it go uncounted until settled saves return a token, so the
//! lifetime counts are then lower bounds.
//!
//! The store owns its two sectors outright, so the newest valid record wins
//! and anything else is a save of its own cut short: losing that save loses
//! at most one save interval of counts, never the totals before it. With no
//! record the totals start at zero. A newer schema version makes the store
//! read-only, and the feeder then reports no lifetime figures rather than
//! totals it cannot keep.

use core::ffi::c_uint;

use wellfed_core::store::slot::{
    DualSlot, Extent, Layout, Pick, Prepare, SlotIo, get_u32, get_u64, newest, put_u32, put_u64,
    seal, target,
};
use wellfed_proto::api::{Counts, Resets};

use crate::stats::{EARLY_SAVES, RESTART_SETTLED_MS, TOTAL_SECTORS, Totals};

pub const VERSION: u16 = 1;
/// The record's size: the totals, padded to a 4-byte program granule.
pub const RECORD_BYTES: usize = 288;
const LAYOUT: Layout = Layout {
    magic: *b"PLFSTA01",
    version: VERSION,
    extent: Extent::Record,
};
const GENERATION_AT: usize = 12;
const COUNTS_AT: usize = 20;
const RESETS_AT: usize = 60;
const UPDATE_AT: usize = 80;
const STORAGE_AT: usize = 84;
const STORAGE_END: usize = STORAGE_AT + 4 * TOTAL_SECTORS;
const EARLY_AT: usize = STORAGE_END;
const FLAGS_AT: usize = EARLY_AT + 1;
const MISSED: u8 = 1;
/// The checksum and the marker, as `seal` places them.
const CRC_AT: usize = RECORD_BYTES - 8;

const _: () = assert!(FLAGS_AT < CRC_AT);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsStatus {
    Unopened,
    /// No record yet: zero totals, writable.
    Empty,
    Loaded,
    /// A newer schema version: no totals, read-only.
    ReadOnly,
    Io,
}

impl StatsStatus {
    /// The status's record word.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unopened => "unopened",
            Self::Empty => "empty",
            Self::Loaded => "loaded",
            Self::ReadOnly => "read-only",
            Self::Io => "io",
        }
    }
}

/// Which save it is, which decides whether it takes an early token or
/// gives one back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveKind {
    /// A boot's own as it starts, or an intentional restart's early in a
    /// boot.
    Early,
    /// The timed one, every `SAVE_MS` of uptime.
    Settled,
    /// An intentional restart's once the boot has run `RESTART_SETTLED_MS`:
    /// it neither takes a token nor gives one back, so a spent budget never
    /// costs an owner's restart its counts, and each such save has a
    /// settled stretch of uptime behind it.
    Restart,
}

impl SaveKind {
    /// The kind of an intentional restart's save after `uptime_ms` of boot.
    pub const fn restart(uptime_ms: u32) -> Self {
        if uptime_ms >= RESTART_SETTLED_MS {
            Self::Restart
        } else {
            Self::Early
        }
    }
}

/// The early tokens in use, and whether a save ever took the last
/// (`missed`, sticky): a boot may have been missed, not that one was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    pub early: u8,
    pub missed: bool,
}

impl Budget {
    /// Whether an early save may go ahead.
    pub const fn allows_early(self) -> bool {
        self.early < EARLY_SAVES
    }

    /// The budget after a save of `kind`.
    const fn after(self, kind: SaveKind) -> Self {
        match kind {
            SaveKind::Early => {
                let early = self.early.saturating_add(1);
                Self {
                    early,
                    missed: self.missed || early >= EARLY_SAVES,
                }
            }
            SaveKind::Settled => Self {
                early: self.early.saturating_sub(1),
                missed: self.missed,
            },
            SaveKind::Restart => self,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StatsStore<I> {
    units: DualSlot<I>,
    pub status: StatsStatus,
    pub generation: u64,
    pub slot: c_uint,
    /// What the newest record holds, or zero.
    pub totals: Totals,
    pub budget: Budget,
}

impl<I> Default for StatsStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: StatsStatus::Unopened,
            generation: 0,
            slot: 0,
            totals: Totals::ZERO,
            budget: Budget {
                early: 0,
                missed: false,
            },
        }
    }
}

fn counts_fields(counts: &Counts) -> [u32; 10] {
    [
        counts.uptime_s,
        counts.feeds,
        counts.feeds_failed,
        counts.portions,
        counts.presses,
        counts.sessions,
        counts.reconnects,
        counts.dhcp_renewals,
        counts.outages,
        counts.battery_s,
    ]
}

fn resets_fields(resets: &Resets) -> [u32; 5] {
    [
        resets.power_on,
        resets.watchdog,
        resets.software,
        resets.brownout,
        resets.other,
    ]
}

/// `values` as consecutive little-endian words from `at`.
fn put_words(p: &mut [u8; RECORD_BYTES], at: usize, values: &[u32]) {
    let Some(field) = p.get_mut(at..at + 4 * values.len()) else {
        return;
    };
    for (bytes, value) in field.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *bytes = value.to_le_bytes();
    }
}

/// `N` consecutive little-endian words from `at`.
fn get_words<const N: usize>(p: &[u8; RECORD_BYTES], at: usize) -> [u32; N] {
    let mut values = [0; N];
    if let Some(field) = p.get(at..at + 4 * N) {
        for (value, bytes) in values.iter_mut().zip(field.as_chunks::<4>().0) {
            *value = u32::from_le_bytes(*bytes);
        }
    }
    values
}

fn encode(generation: u64, totals: &Totals, budget: Budget) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u64::<GENERATION_AT, _>(&mut p, generation);
    put_words(&mut p, COUNTS_AT, &counts_fields(&totals.counts));
    put_words(&mut p, RESETS_AT, &resets_fields(&totals.resets));
    put_u32::<UPDATE_AT, _>(&mut p, totals.update);
    put_words(&mut p, STORAGE_AT, &totals.storage);
    p[EARLY_AT] = budget.early;
    p[FLAGS_AT] = if budget.missed { MISSED } else { 0 };
    seal(&mut p);
    p
}

/// The generation, the totals and the budget of a record whose header and
/// checksum hold.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u64, Totals, Budget)> {
    let generation = get_u64::<GENERATION_AT, _>(p);
    // Unused bytes and bits must be zero, so one encoding exists per value.
    if generation == 0
        || p[EARLY_AT] > EARLY_SAVES
        || p[FLAGS_AT] & !MISSED != 0
        || p[FLAGS_AT + 1..CRC_AT].iter().any(|&b| b != 0)
    {
        return None;
    }
    let budget = Budget {
        early: p[EARLY_AT],
        missed: p[FLAGS_AT] & MISSED != 0,
    };

    let [
        uptime_s,
        feeds,
        feeds_failed,
        portions,
        presses,
        sessions,
        reconnects,
        dhcp_renewals,
        outages,
        battery_s,
    ] = get_words::<10>(p, COUNTS_AT);
    let [power_on, watchdog, software, brownout, other] = get_words::<5>(p, RESETS_AT);
    let totals = Totals {
        counts: Counts {
            uptime_s,
            feeds,
            feeds_failed,
            portions,
            presses,
            sessions,
            reconnects,
            dhcp_renewals,
            outages,
            battery_s,
        },
        resets: Resets {
            power_on,
            watchdog,
            software,
            brownout,
            other,
        },
        storage: get_words::<TOTAL_SECTORS>(p, STORAGE_AT),
        update: get_u32::<UPDATE_AT, _>(p),
    };
    Some((generation, totals, budget))
}

impl<I: SlotIo> StatsStore<I> {
    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I) -> StatsStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = StatsStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.units.scan(0, decode), self.units.scan(1, decode)) else {
            self.status = StatsStatus::Io;
            return self.status;
        };

        self.status = match newest(a, b, |&(generation, _, _)| generation) {
            Pick::Future => StatsStatus::ReadOnly,
            Pick::Newest(slot, (generation, totals, budget)) => {
                self.slot = slot;
                self.generation = generation;
                self.totals = totals;
                self.budget = budget;
                StatsStatus::Loaded
            }
            // The two sectors are this store's alone, so anything else there
            // is a save of ours cut short: zero totals, still writable.
            Pick::Nothing => StatsStatus::Empty,
        };
        self.status
    }

    /// The totals can be read and saved: a record or none, in this
    /// build's version.
    pub fn writable(&self) -> bool {
        matches!(self.status, StatsStatus::Empty | StatsStatus::Loaded)
    }

    /// The unit the next save erases and writes, so the caller can count
    /// that erase in the totals it saves.
    pub fn next_slot(&self) -> c_uint {
        target(self.generation, self.slot)
    }

    /// Whether a save of `kind` may go ahead: a writable store, and for an
    /// early save, budget left.
    pub fn may_save(&self, kind: SaveKind) -> bool {
        self.writable() && (kind != SaveKind::Early || self.budget.allows_early())
    }

    /// Store `totals` to the other slot by the torn-write rule, spending or
    /// refilling the early budget by `kind`. Refused without a write when
    /// [`Self::may_save`] is false. A failed write makes the store
    /// unwritable for the rest of the boot (no retry), and keeps the
    /// previous record as the newest on flash.
    pub fn save(&mut self, totals: &Totals, kind: SaveKind) -> bool {
        if !self.may_save(kind) || !self.units.writable() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = self.next_slot();
        let budget = self.budget.after(kind);
        if !self
            .units
            .write(slot, &encode(generation, totals, budget), Prepare::Erase)
        {
            self.status = StatsStatus::Io;
            return false;
        }
        self.slot = slot;
        self.generation = generation;
        self.totals = *totals;
        self.budget = budget;
        true
    }
}
