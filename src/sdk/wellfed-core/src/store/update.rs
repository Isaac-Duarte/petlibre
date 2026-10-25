//! The update store: what survives between the two banks' halves of an
//! update, and the anti-rollback floor.
//!
//! - **The floor** (`min_version`): the highest version a build confirmed
//!   at. An offer at or below it needs a downgrade statement. It only rises.
//! - **The candidate:** the manifest bank 1 gave way for (`Pending`), so
//!   bank 0 installs that one and no other, then the one bank 0 activated
//!   (`Activated`), which the new build matches against its own identity
//!   when it confirms.
//! - **Keys:** the release slot a key statement filled, and every key id
//!   a statement retired. A retired id is never forgotten.
//! - **Bank 0's image:** the build that last ran there, its length and its
//!   SHA-256, which bank 0 records at boot. Bank 1 gives way only to a bank
//!   0 that still holds exactly that image, so a give-way never lands on a
//!   build that cannot take the offer.
//!
//! One 256-byte record, written whole to the other of two erase units on
//! every change, newest valid wins, as the client store does:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..8 | magic `PLFUPD01` |
//! | 8..12 | version (u16), record length (u16) |
//! | 12..20 | generation (u64) |
//! | 20..24 | `min_version` |
//! | 24 | state: 0 none, 1 pending, 2 activated |
//! | 25 | the candidate's image kind (0 with no candidate) |
//! | 26 | 1 when the release slot is filled |
//! | 27 | retired key ids held |
//! | 28..32 | the candidate's version |
//! | 32..64 | the candidate's manifest digest |
//! | 64..96 | the candidate's build ID |
//! | 96..128 | the release slot's key |
//! | 128..160 | [`REVOKED`] retired key ids |
//! | 160 | 1 when bank 0's image is recorded |
//! | 161 | 1 when bank 0's build has a built-in key |
//! | 164..168 | bank 0's image length |
//! | 168..200 | bank 0's build ID |
//! | 200..232 | bank 0's image SHA-256 |
//! | 232..240 | bank 0's built-in key id |
//! | 248 | CRC-32/ISO-HDLC of bytes 0..248 |
//! | 252 | `CMIT`, programmed last |
//!
//! Unused bytes are zero. A torn write leaves the older record, which is
//! always consistent: the floor and the retired ids it holds are ones a
//! newer record only extends. Storage formats stay readable by the bank-0
//! build ([OTA design](../../../../../docs/firmware/ota-design.md)), so a
//! change to this layout ships a bank-0 refresh first. The factory reset
//! leaves the store alone: a reset must not reopen a downgrade.

use core::ffi::c_uint;

use crate::ota::offer::{ImageKind, KeyId, key_id};
use crate::store::slot::{
    Bound, COMMIT, LENGTH_AT, SlotIo, VERSION_AT, crc32, get_u16, get_u32, get_u64, put_u16,
    put_u32, put_u64,
};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFUPD01";
/// Retired key ids the store keeps.
pub const REVOKED: usize = 4;
/// The whole record.
pub const RECORD_BYTES: usize = 256;
const RECORD_LEN: u16 = 256;

const MIN_AT: usize = 20;
const STATE_AT: usize = 24;
const KIND_AT: usize = 25;
const HAS_ADDED_AT: usize = 26;
const REVOKED_COUNT_AT: usize = 27;
const VERSION_FIELD_AT: usize = 28;
const DIGEST_AT: usize = 32;
const BUILD_AT: usize = 64;
const ADDED_AT: usize = 96;
const REVOKED_AT: usize = 128;
const HAS_BANK0_AT: usize = 160;
const BANK0_LEN_AT: usize = 164;
const BANK0_BUILD_AT: usize = 168;
const BANK0_SHA_AT: usize = 200;
const HAS_BANK0_KEY_AT: usize = 161;
const BANK0_KEY_AT: usize = 232;
const BANK0_END: usize = 240;
const CRC_AT: usize = 248;
const COMMIT_AT: usize = 252;
const COMMIT_OFFSET: u32 = 252;

const _: () = assert!(REVOKED_AT + REVOKED * 8 == HAS_BANK0_AT);
const _: () = assert!(COMMIT_AT + COMMIT.len() == RECORD_BYTES);
const _: () = assert!(COMMIT_OFFSET as usize == COMMIT_AT && RECORD_LEN as usize == RECORD_BYTES);

/// One manifest the update is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// `ota::offer::manifest_digest` of the manifest.
    pub digest: [u8; 32],
    pub version: u32,
    pub build_id: [u8; 32],
    pub kind: ImageKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    None,
    /// Bank 1 gave way for it, or bank 0 accepted it: bank 0 installs only
    /// this manifest.
    Pending(Candidate),
    /// Bank 0 activated it; the build that confirms with this identity
    /// raises the floor to its version.
    Activated(Candidate),
}

impl State {
    const fn code(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Pending(_) => 1,
            Self::Activated(_) => 2,
        }
    }

    pub const fn candidate(self) -> Option<Candidate> {
        match self {
            Self::None => None,
            Self::Pending(c) | Self::Activated(c) => Some(c),
        }
    }
}

/// The image bank 0 last ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bank0 {
    pub build_id: [u8; 32],
    pub len: u32,
    pub sha256: [u8; 32],
    /// The id of the update key built into it, so bank 1 never gives way
    /// for an offer bank 0 would find no key for.
    pub key: Option<KeyId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub min_version: u32,
    pub state: State,
    /// The release slot.
    pub added: Option<[u8; 32]>,
    revoked: [KeyId; REVOKED],
    revoked_len: u8,
    pub bank0: Option<Bank0>,
}

impl Update {
    /// The retired key ids, oldest first.
    pub fn revoked(&self) -> &[KeyId] {
        &self.revoked[..usize::from(self.revoked_len).min(REVOKED)]
    }

    pub fn is_revoked(&self, id: &KeyId) -> bool {
        self.revoked().contains(id)
    }

    /// The release slot's key, unless a statement retired it.
    pub fn added_key(&self) -> Option<[u8; 32]> {
        self.added.filter(|key| !self.is_revoked(&key_id(key)))
    }

    /// `next` keeps every promise this record made: the floor and the
    /// retired ids only grow.
    fn kept_by(&self, next: &Self) -> bool {
        next.min_version >= self.min_version && self.revoked().iter().all(|id| next.is_revoked(id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Unopened,
    /// No record yet: writable, floor 0.
    Empty,
    Loaded,
    /// A newer schema version: read-only.
    ReadOnly,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// Unopened, read-only or failed earlier.
    NotWritable,
    /// Every retired-id slot is taken.
    Full,
    /// The release slot holds a key no statement retired.
    Occupied,
    /// That key is retired.
    Revoked,
    /// No activated candidate names this build.
    NotActivated,
    /// The write failed; the previous record stands in memory.
    Io,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a scan result on the stack once per unit, never stored"
)]
enum Scan {
    Erased,
    Valid(u64, Update),
    Future,
    Invalid,
}

#[derive(Clone, Copy)]
pub struct UpdateStore<I> {
    io: Bound<I>,
    pub status: UpdateStatus,
    pub generation: u64,
    pub slot: c_uint,
    record: Update,
}

impl<I> core::fmt::Debug for UpdateStore<I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UpdateStore")
            .field("status", &self.status)
            .field("generation", &self.generation)
            .field("slot", &self.slot)
            .field("min_version", &self.record.min_version)
            .finish_non_exhaustive()
    }
}

impl<I> Default for UpdateStore<I> {
    fn default() -> Self {
        Self {
            io: Bound::NONE,
            status: UpdateStatus::Unopened,
            generation: 0,
            slot: 0,
            record: Update::default(),
        }
    }
}

fn put_candidate(p: &mut [u8; RECORD_BYTES], candidate: &Candidate) {
    p[KIND_AT] = candidate.kind.code();
    put_u32(&mut p[VERSION_FIELD_AT..], candidate.version);
    p[DIGEST_AT..BUILD_AT].copy_from_slice(&candidate.digest);
    p[BUILD_AT..ADDED_AT].copy_from_slice(&candidate.build_id);
}

fn encode(generation: u64, record: &Update) -> [u8; RECORD_BYTES] {
    let mut p = [0u8; RECORD_BYTES];
    p[..8].copy_from_slice(&MAGIC);
    put_u16(&mut p[VERSION_AT..], VERSION);
    put_u16(&mut p[LENGTH_AT..], RECORD_LEN);
    put_u64(&mut p[12..], generation);
    put_u32(&mut p[MIN_AT..], record.min_version);
    p[STATE_AT] = record.state.code();
    if let Some(candidate) = record.state.candidate() {
        put_candidate(&mut p, &candidate);
    }
    if let Some(key) = &record.added {
        p[HAS_ADDED_AT] = 1;
        p[ADDED_AT..REVOKED_AT].copy_from_slice(key);
    }
    let revoked = record.revoked();
    p[REVOKED_COUNT_AT] = record.revoked_len;
    for (out, id) in p[REVOKED_AT..HAS_BANK0_AT]
        .as_chunks_mut::<8>()
        .0
        .iter_mut()
        .zip(revoked)
    {
        *out = *id;
    }
    if let Some(bank0) = &record.bank0 {
        p[HAS_BANK0_AT] = 1;
        put_u32(&mut p[BANK0_LEN_AT..], bank0.len);
        p[BANK0_BUILD_AT..BANK0_SHA_AT].copy_from_slice(&bank0.build_id);
        p[BANK0_SHA_AT..BANK0_KEY_AT].copy_from_slice(&bank0.sha256);
        if let Some(key) = &bank0.key {
            p[HAS_BANK0_KEY_AT] = 1;
            p[BANK0_KEY_AT..BANK0_END].copy_from_slice(key);
        }
    }
    let crc = crc32(&p[..CRC_AT]);
    put_u32(&mut p[CRC_AT..], crc);
    p[COMMIT_AT..].copy_from_slice(&COMMIT);
    p
}

fn field<const N: usize>(p: &[u8]) -> [u8; N] {
    let mut out = [0; N];
    out.copy_from_slice(&p[..N]);
    out
}

fn zero(p: &[u8]) -> bool {
    p.iter().all(|&b| b == 0)
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
    if version != VERSION || generation == 0 || !zero(&p[BANK0_END..CRC_AT]) {
        return Scan::Invalid;
    }
    decode_fields(p).map_or(Scan::Invalid, |record| Scan::Valid(generation, record))
}

fn decode_fields(p: &[u8; RECORD_BYTES]) -> Option<Update> {
    let candidate = Candidate {
        digest: field(&p[DIGEST_AT..]),
        version: get_u32(&p[VERSION_FIELD_AT..]),
        build_id: field(&p[BUILD_AT..]),
        kind: ImageKind::Replacement,
    };
    let state = match p[STATE_AT] {
        0 if p[KIND_AT] == 0 && zero(&p[VERSION_FIELD_AT..ADDED_AT]) => State::None,
        1 | 2 => {
            let candidate = Candidate {
                kind: ImageKind::from_byte(p[KIND_AT])?,
                ..candidate
            };
            if p[STATE_AT] == 1 {
                State::Pending(candidate)
            } else {
                State::Activated(candidate)
            }
        }
        _ => return None,
    };

    let added = match p[HAS_ADDED_AT] {
        0 if zero(&p[ADDED_AT..REVOKED_AT]) => None,
        1 => Some(field(&p[ADDED_AT..])),
        _ => return None,
    };

    let revoked_len = p[REVOKED_COUNT_AT];
    let held = usize::from(revoked_len);
    if held > REVOKED {
        return None;
    }
    let mut revoked = [[0; 8]; REVOKED];
    for (index, (out, id)) in revoked
        .iter_mut()
        .zip(p[REVOKED_AT..HAS_BANK0_AT].as_chunks::<8>().0)
        .enumerate()
    {
        if index >= held && !zero(id) {
            return None;
        }
        *out = *id;
    }

    let bank0 = match p[HAS_BANK0_AT] {
        0 if zero(&p[HAS_BANK0_AT..BANK0_END]) => None,
        1 if zero(&p[HAS_BANK0_KEY_AT + 1..BANK0_LEN_AT]) => Some(Bank0 {
            build_id: field(&p[BANK0_BUILD_AT..]),
            len: get_u32(&p[BANK0_LEN_AT..]),
            sha256: field(&p[BANK0_SHA_AT..]),
            key: match p[HAS_BANK0_KEY_AT] {
                0 if zero(&p[BANK0_KEY_AT..BANK0_END]) => None,
                1 => Some(field(&p[BANK0_KEY_AT..])),
                _ => return None,
            },
        }),
        _ => return None,
    };

    Some(Update {
        min_version: get_u32(&p[MIN_AT..]),
        state,
        added,
        revoked,
        revoked_len,
        bank0,
    })
}

impl<I: SlotIo> UpdateStore<I> {
    fn scan(&self, slot: c_uint) -> Option<Scan> {
        let mut record = [0u8; RECORD_BYTES];
        self.io.read(slot, 0, &mut record).then(|| decode(&record))
    }

    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I) -> UpdateStatus {
        let unit = io.unit_bytes();
        *self = Self {
            io: Bound::new(io),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = UpdateStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = UpdateStatus::Io;
            return self.status;
        };

        let pick = match (a, b) {
            (Scan::Valid(ga, ra), Scan::Valid(gb, rb)) => {
                Some(if ga >= gb { (0, ga, ra) } else { (1, gb, rb) })
            }
            (Scan::Valid(g, r), _) => Some((0, g, r)),
            (_, Scan::Valid(g, r)) => Some((1, g, r)),
            _ => None,
        };
        self.status = if a == Scan::Future || b == Scan::Future {
            UpdateStatus::ReadOnly
        } else if let Some((slot, generation, record)) = pick {
            self.slot = slot;
            self.generation = generation;
            self.record = record;
            UpdateStatus::Loaded
        } else {
            UpdateStatus::Empty
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, UpdateStatus::Empty | UpdateStatus::Loaded)
    }

    /// Whether the floor is known. A store that could not be read, or one
    /// a newer schema wrote, has no floor this build can trust, so the
    /// update path refuses every offer until a build that reads it runs.
    pub fn readable(&self) -> bool {
        matches!(self.status, UpdateStatus::Empty | UpdateStatus::Loaded)
    }

    pub fn record(&self) -> &Update {
        &self.record
    }

    /// Bank 0 installs only `candidate`.
    pub fn set_pending(&mut self, candidate: Candidate) -> Result<(), UpdateError> {
        self.save(&Update {
            state: State::Pending(candidate),
            ..self.record
        })
    }

    /// Bank 0 activated `candidate`: written before the marker.
    pub fn set_activated(&mut self, candidate: Candidate) -> Result<(), UpdateError> {
        self.save(&Update {
            state: State::Activated(candidate),
            ..self.record
        })
    }

    /// Forget the candidate (the owner cancelled it).
    pub fn clear(&mut self) -> Result<(), UpdateError> {
        if self.record.state == State::None {
            return Ok(());
        }
        self.save(&Update {
            state: State::None,
            ..self.record
        })
    }

    /// The build `build_id` confirmed: if bank 0 activated it, raise the
    /// floor to its version and forget the candidate. Its version.
    pub fn confirm(&mut self, build_id: &[u8; 32]) -> Result<u32, UpdateError> {
        let State::Activated(candidate) = self.record.state else {
            return Err(UpdateError::NotActivated);
        };
        if candidate.build_id != *build_id {
            return Err(UpdateError::NotActivated);
        }
        self.save(&Update {
            min_version: self.record.min_version.max(candidate.version),
            state: State::None,
            ..self.record
        })?;
        Ok(candidate.version)
    }

    /// Retire `id` for good; retiring it again changes nothing.
    pub fn revoke(&mut self, id: &KeyId) -> Result<(), UpdateError> {
        if self.record.is_revoked(id) {
            return Ok(());
        }
        let held = usize::from(self.record.revoked_len);
        if held >= REVOKED {
            return Err(UpdateError::Full);
        }
        let mut next = self.record;
        next.revoked[held] = *id;
        next.revoked_len += 1;
        self.save(&next)
    }

    /// Fill the release slot with `key`, when it is empty or holds a
    /// retired key.
    pub fn add_key(&mut self, key: &[u8; 32]) -> Result<(), UpdateError> {
        if self.record.is_revoked(&key_id(key)) {
            return Err(UpdateError::Revoked);
        }
        match self.record.added_key() {
            Some(current) if current == *key => return Ok(()),
            Some(_) => return Err(UpdateError::Occupied),
            None => {}
        }
        self.save(&Update {
            added: Some(*key),
            ..self.record
        })
    }

    /// Record the image bank 0 runs; nothing is written when it is already
    /// recorded, so a boot costs no erase.
    pub fn set_bank0(&mut self, bank0: Bank0) -> Result<(), UpdateError> {
        if self.record.bank0 == Some(bank0) {
            return Ok(());
        }
        self.save(&Update {
            bank0: Some(bank0),
            ..self.record
        })
    }

    /// Write `next` to the other unit: erase, program, verify, commit,
    /// verify. A failure keeps the previous record in memory. A record that
    /// would lower the floor or forget a retired id is never written.
    fn save(&mut self, next: &Update) -> Result<(), UpdateError> {
        if !self.writable() || !self.io.writable() || !self.record.kept_by(next) {
            return Err(UpdateError::NotWritable);
        }
        let generation = self.generation + 1;
        let slot = if self.generation == 0 {
            0
        } else {
            1 - self.slot
        };
        let bytes = encode(generation, next);
        let mut checked = [0u8; RECORD_BYTES];
        let ok = self.io.erase(slot)
            && self.io.read(slot, 0, &mut checked)
            && checked.iter().all(|&b| b == u8::MAX)
            && self.io.program(slot, 0, &bytes[..COMMIT_AT])
            && self.io.program(slot, COMMIT_OFFSET, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;
        if !ok {
            self.status = UpdateStatus::Io;
            return Err(UpdateError::Io);
        }
        self.slot = slot;
        self.generation = generation;
        self.record = *next;
        Ok(())
    }
}
