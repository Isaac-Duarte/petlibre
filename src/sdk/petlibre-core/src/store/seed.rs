//! The random generator's seed, carried across boots.
//!
//! Two alternating 128-byte records on the shared slot layout: magic
//! `PLFSEED1`, version, record size, generation, the 32-byte seed, zero
//! padding, CRC-32/ISO-HDLC of bytes 0..120 at 120 and `CMIT` at 124. The
//! newest valid record wins and a torn newer slot falls back to the older
//! one. The sectors are this store's alone, so an unreadable record is a
//! save of ours cut short; only a newer schema makes the store read-only.
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
    COMMIT, COMMIT_AT, COMMIT_OFFSET, CRC_AT, JournalIo, LENGTH_AT, RECORD_BYTES, RECORD_LEN,
    VERSION_AT, crc32, get_u16, get_u32, get_u64, put_u16, put_u32, put_u64,
};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFSEED1";
const SEED_AT: usize = 20;
const SEED_END: usize = SEED_AT + 32;
/// Personalization for the firmware's DRBG.
const PERSONALIZATION: &[u8] = b"petlibre-fw45-drbg";

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Erased,
    Valid(u64, [u8; 32]),
    Future,
    Invalid,
}

#[derive(Clone, Copy)]
pub struct SeedStore {
    io: JournalIo,
    pub status: SeedStatus,
    pub generation: u64,
    pub slot: c_uint,
    seed: Option<[u8; 32]>,
}

impl core::fmt::Debug for SeedStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SeedStore")
            .field("status", &self.status)
            .field("generation", &self.generation)
            .field("slot", &self.slot)
            .finish_non_exhaustive()
    }
}

impl Default for SeedStore {
    fn default() -> Self {
        Self {
            io: JournalIo::default(),
            status: SeedStatus::Unopened,
            generation: 0,
            slot: 0,
            seed: None,
        }
    }
}

fn encode(generation: u64, seed: &[u8; 32]) -> [u8; RECORD_BYTES] {
    let mut p = [0u8; RECORD_BYTES];
    p[..8].copy_from_slice(&MAGIC);
    put_u16(&mut p[VERSION_AT..], VERSION);
    put_u16(&mut p[LENGTH_AT..], RECORD_LEN);
    put_u64(&mut p[12..], generation);
    p[SEED_AT..SEED_END].copy_from_slice(seed);
    let crc = crc32(&p[..CRC_AT]);
    put_u32(&mut p[CRC_AT..], crc);
    p[COMMIT_AT..].copy_from_slice(&COMMIT);
    p
}

fn decode(p: &[u8; RECORD_BYTES]) -> Scan {
    if p.iter().all(|&b| b == u8::MAX) {
        return Scan::Erased;
    }
    if p[..8] != MAGIC
        || usize::from(get_u16(&p[LENGTH_AT..])) != RECORD_BYTES
        || p[COMMIT_AT..] != COMMIT
        || get_u32(&p[CRC_AT..]) != crc32(&p[..CRC_AT])
    {
        return Scan::Invalid;
    }
    let version = get_u16(&p[VERSION_AT..]);
    if version > VERSION {
        return Scan::Future;
    }
    let generation = get_u64(&p[12..]);
    if version != VERSION || generation == 0 || p[SEED_END..CRC_AT].iter().any(|&b| b != 0) {
        return Scan::Invalid;
    }
    let mut seed = [0; 32];
    seed.copy_from_slice(&p[SEED_AT..SEED_END]);
    Scan::Valid(generation, seed)
}

impl SeedStore {
    fn scan(&self, slot: c_uint) -> Option<Scan> {
        let mut record = [0u8; RECORD_BYTES];
        self.io.read(slot, 0, &mut record).then(|| decode(&record))
    }

    /// Bind to two exclusive erase units and load the newest valid seed.
    ///
    /// # Safety
    /// The I/O callbacks must be sound for `io.context` while the store is used.
    #[allow(unsafe_code)]
    pub unsafe fn open(&mut self, io: JournalIo) -> SeedStatus {
        *self = Self {
            io,
            ..Self::default()
        };
        if io.read.is_none() || (io.unit_bytes as usize) < RECORD_BYTES {
            self.status = SeedStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = SeedStatus::Io;
            return self.status;
        };

        let pick = match (a, b) {
            (Scan::Valid(ga, sa), Scan::Valid(gb, sb)) => {
                Some(if ga >= gb { (0, ga, sa) } else { (1, gb, sb) })
            }
            (Scan::Valid(g, s), _) => Some((0, g, s)),
            (_, Scan::Valid(g, s)) => Some((1, g, s)),
            _ => None,
        };
        self.status = if a == Scan::Future || b == Scan::Future {
            SeedStatus::ReadOnly
        } else if let Some((slot, generation, seed)) = pick {
            self.slot = slot;
            self.generation = generation;
            self.seed = Some(seed);
            SeedStatus::Loaded
        } else {
            SeedStatus::Empty
        };
        self.status
    }

    pub fn seed(&self) -> Option<&[u8; 32]> {
        self.seed.as_ref()
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, SeedStatus::Empty | SeedStatus::Loaded)
    }

    /// Store `seed` to the other slot: erase, program, verify, commit,
    /// verify. A failure keeps the previous seed in memory.
    pub fn save(&mut self, seed: &[u8; 32]) -> bool {
        if !self.writable() || self.io.program.is_none() || self.io.erase.is_none() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = if self.generation == 0 {
            0
        } else {
            1 - self.slot
        };
        let bytes = encode(generation, seed);
        let mut checked = [0u8; RECORD_BYTES];
        let ok = self.io.erase(slot)
            && self.io.read(slot, 0, &mut checked)
            && checked.iter().all(|&b| b == u8::MAX)
            && self.io.program(slot, 0, &bytes[..COMMIT_AT])
            && self.io.program(slot, COMMIT_OFFSET, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;
        if !ok {
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
pub fn start(store: &mut SeedStore, fresh: &[u8; 32]) -> Result<Drbg, Error> {
    let stored = store.seed.unwrap_or([0; 32]);
    let mut drbg = Drbg::new(fresh, &stored, PERSONALIZATION);
    let next = drbg.bytes()?;
    store.save(&next);
    Ok(drbg)
}
