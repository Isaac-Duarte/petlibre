//! The portion calibration on NOR: what one portion weighs, as the owner
//! measured it.
//!
//! A 128-byte record in the dual-slot store
//! (`wellfed_core::store::slot`), as the time settings are: magic
//! `PLFCAL01`, version, record size, generation, the portion's weight in
//! milligrams at 20..24 (little-endian, zero when none is measured), zero
//! padding, CRC-32/ISO-HDLC of bytes 0..120 at 120 and the `CMIT` commit
//! marker at 124.
//!
//! Its two sectors were free before this store, so a build without it
//! never reads or writes them, and the settings record stays as it was: a
//! bank that predates the store runs on its own settings and ignores
//! these. The newest valid record wins; a torn or invalid newer slot falls
//! back to the older one. With no record no weight is known. Only a newer
//! schema version makes the store read-only, with no weight.
//!
//! The bowl mode is not stored here: a build runs the one mode it was
//! verified for, and the settings record's bowl byte stays the single
//! bowl it has always held.

use core::ffi::c_uint;

use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u64, newest,
    put_u64, seal, target,
};
use wellfed_device_api::mailbox::Calibration;

pub const VERSION: u16 = 1;
const LAYOUT: Layout = Layout {
    magic: *b"PLFCAL01",
    version: VERSION,
    extent: Extent::Record,
};
const GENERATION_AT: usize = 12;
const PORTION_AT: usize = 20;
const PORTION_END: usize = PORTION_AT + 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationStatus {
    Unopened,
    /// No record yet: nothing measured, writable.
    Empty,
    Loaded,
    /// A newer schema version: nothing measured, read-only.
    ReadOnly,
    Io,
}

impl CalibrationStatus {
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

#[derive(Clone, Copy, Debug)]
pub struct CalibrationStore<I> {
    units: DualSlot<I>,
    pub status: CalibrationStatus,
    pub generation: u64,
    pub slot: c_uint,
    pub calibration: Calibration,
}

impl<I> Default for CalibrationStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: CalibrationStatus::Unopened,
            generation: 0,
            slot: 0,
            calibration: Calibration::default(),
        }
    }
}

fn encode(generation: u64, calibration: &Calibration) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u64::<GENERATION_AT, _>(&mut p, generation);
    let mg = calibration.portion_mg.unwrap_or(0);
    p[PORTION_AT..PORTION_END].copy_from_slice(&mg.to_le_bytes());
    seal(&mut p);
    p
}

/// The generation and the calibration of a record whose header and
/// checksum hold. A weight outside the limits is no record of ours.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u64, Calibration)> {
    let generation = get_u64::<GENERATION_AT, _>(p);
    let bytes: [u8; 4] = p[PORTION_AT..PORTION_END].try_into().ok()?;
    let calibration = Calibration {
        portion_mg: match u32::from_le_bytes(bytes) {
            0 => None,
            mg => Some(mg),
        },
    };

    // Unused bytes must be zero, so one encoding exists per value.
    if generation == 0 || p[PORTION_END..CRC_AT].iter().any(|&b| b != 0) || !calibration.valid() {
        return None;
    }
    Some((generation, calibration))
}

const _: () = assert!(PORTION_END <= CRC_AT);

impl<I: SlotIo> CalibrationStore<I> {
    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I) -> CalibrationStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = CalibrationStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.units.scan(0, decode), self.units.scan(1, decode)) else {
            self.status = CalibrationStatus::Io;
            return self.status;
        };

        self.status = match newest(a, b, |&(generation, _)| generation) {
            Pick::Future => CalibrationStatus::ReadOnly,
            Pick::Newest(slot, (generation, calibration)) => {
                self.slot = slot;
                self.generation = generation;
                self.calibration = calibration;
                CalibrationStatus::Loaded
            }
            // The two sectors are this store's alone, so anything else there
            // is a save of ours cut short: nothing measured, still writable.
            Pick::Nothing => CalibrationStatus::Empty,
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(
            self.status,
            CalibrationStatus::Empty | CalibrationStatus::Loaded
        )
    }

    /// Store `calibration` to the other slot by the torn-write rule. A
    /// failure keeps the previous calibration in memory; one outside the
    /// limits is refused before any write.
    pub fn save(&mut self, calibration: &Calibration) -> bool {
        if !self.writable() || !self.units.writable() || !calibration.valid() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = target(self.generation, self.slot);
        if !self
            .units
            .write(slot, &encode(generation, calibration), Prepare::Erase)
        {
            self.status = CalibrationStatus::Io;
            return false;
        }
        self.slot = slot;
        self.generation = generation;
        self.calibration = *calibration;
        true
    }
}
