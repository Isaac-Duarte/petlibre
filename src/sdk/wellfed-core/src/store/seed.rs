//! The random generator's seed, carried across boots.
//!
//! A 128-byte record in the dual-slot store ([`slot`](crate::store::slot)):
//! magic `PLFSEED1`, version, record size, generation, the 32-byte seed,
//! zero padding, CRC-32/ISO-HDLC of bytes 0..120 at 120 and `CMIT` at 124.
//! The newest valid record wins and a torn newer slot falls back to the
//! older one. The sectors are this store's alone, so an unreadable record
//! is a save of ours cut short; only a newer schema makes the store
//! read-only.
//!
//! [`start`] is the only intended writer: each boot instantiates the DRBG
//! from fresh pool input with the stored seed as its nonce, then stores a
//! new seed drawn from it before any key or nonce is handed out. A boot
//! whose fresh input repeats a previous one still starts from a seed no
//! earlier boot used. The seed sits in plain flash, as every secret on this
//! board does; reading it lets an attacker predict nothing once a boot has
//! mixed fresh input in.

use core::ffi::c_uint;

use crate::rng::{Drbg, Error};
use crate::store::slot::{
    CRC_AT, DualSlot, Extent, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u64, newest,
    put_u64, seal, target,
};

pub const VERSION: u16 = 1;
const LAYOUT: Layout = Layout {
    magic: *b"PLFSEED1",
    version: VERSION,
    extent: Extent::Record,
};
const GENERATION_AT: usize = 12;
const SEED_AT: usize = 20;
const SEED_END: usize = SEED_AT + 32;
/// Personalization for the firmware's DRBG.
const PERSONALIZATION: &[u8] = b"wellfed-fw45-drbg";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedStatus {
    Unopened,
    /// No seed yet (a first boot, or every save was cut short).
    Empty,
    Loaded,
    /// A newer schema version: read-only.
    ReadOnly,
    Io,
}

#[derive(Clone, Copy)]
pub struct SeedStore<I> {
    units: DualSlot<I>,
    pub status: SeedStatus,
    pub generation: u64,
    pub slot: c_uint,
    seed: Option<[u8; 32]>,
}

impl<I> core::fmt::Debug for SeedStore<I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SeedStore")
            .field("status", &self.status)
            .field("generation", &self.generation)
            .field("slot", &self.slot)
            .finish_non_exhaustive()
    }
}

impl<I> Default for SeedStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: SeedStatus::Unopened,
            generation: 0,
            slot: 0,
            seed: None,
        }
    }
}

fn encode(generation: u64, seed: &[u8; 32]) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u64::<GENERATION_AT, _>(&mut p, generation);
    p[SEED_AT..SEED_END].copy_from_slice(seed);
    seal(&mut p);
    p
}

/// The generation and the seed.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u64, [u8; 32])> {
    let generation = get_u64::<GENERATION_AT, _>(p);
    if generation == 0 || p[SEED_END..CRC_AT].iter().any(|&b| b != 0) {
        return None;
    }
    let mut seed = [0; 32];
    seed.copy_from_slice(&p[SEED_AT..SEED_END]);
    Some((generation, seed))
}

impl<I: SlotIo> SeedStore<I> {
    /// Bind to two exclusive erase units and load the newest valid seed.
    pub fn open(&mut self, io: I) -> SeedStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = SeedStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.units.scan(0, decode), self.units.scan(1, decode)) else {
            self.status = SeedStatus::Io;
            return self.status;
        };

        self.status = match newest(a, b, |&(generation, _)| generation) {
            Pick::Future => SeedStatus::ReadOnly,
            Pick::Newest(slot, (generation, seed)) => {
                self.slot = slot;
                self.generation = generation;
                self.seed = Some(seed);
                SeedStatus::Loaded
            }
            Pick::Nothing => SeedStatus::Empty,
        };
        self.status
    }

    pub fn seed(&self) -> Option<&[u8; 32]> {
        self.seed.as_ref()
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, SeedStatus::Empty | SeedStatus::Loaded)
    }

    /// Store `seed` to the other slot by the torn-write rule. A failure
    /// keeps the previous seed in memory.
    pub fn save(&mut self, seed: &[u8; 32]) -> bool {
        if !self.writable() || !self.units.writable() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = target(self.generation, self.slot);
        if !self
            .units
            .write(slot, &encode(generation, seed), Prepare::Erase)
        {
            self.status = SeedStatus::Io;
            return false;
        }
        self.slot = slot;
        self.generation = generation;
        self.seed = Some(*seed);
        true
    }
}

/// The boot's generator: `fresh` pool input with the stored seed, if any,
/// as the nonce; then the next boot's seed is stored. The caller hands out
/// nothing before this returns.
///
/// A failed save still returns the generator, since `fresh` is health-tested
/// input of full claimed entropy; the store's status reports the failure.
pub fn start<I: SlotIo>(store: &mut SeedStore<I>, fresh: &[u8; 32]) -> Result<Drbg, Error> {
    let stored = store.seed.unwrap_or([0; 32]);
    let mut drbg = Drbg::new(fresh, &stored, PERSONALIZATION);
    let next = drbg.bytes()?;
    store.save(&next);
    Ok(drbg)
}
