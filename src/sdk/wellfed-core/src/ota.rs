//! Update logic (design: docs/firmware/ota-design.md): the manifest's
//! fields, the bank-1 sector writer and the boot-trial decision. Offers over
//! the device API carry [`offer`]'s signed version 2 manifest.
//!
//! Sans-IO. The board port fetches chunks, runs each erase/program/readback
//! inside a flash lease and writes the marker last; this module only decides
//! what to write and whether the result may be activated.

pub mod image;
pub mod install;
pub mod offer;

use sha2::{Digest, Sha256};

/// The retired version 1 manifest (148 bytes), which builds fetching
/// updates over MQTT took. An offer refuses its magic, and a signer still
/// reads the versions of the ones on disk.
pub(crate) const RETIRED_MAGIC: [u8; 8] = *b"PLOTA1\0\0";
const RETIRED_LEN: usize = 148;

/// One NOR erase sector, as flash offsets measure it and as buffers do.
pub const SECTOR_BYTES: u32 = 0x1000;
pub const SECTOR: usize = SECTOR_BYTES as usize;

/// The factory loader's validity marker at the start of a bank.
pub const MARKER: [u8; 8] = *b"81958711";
/// Bank 1 starts at 0x186000; the reviewed footprint ends at 0x2d6000.
pub const BANK1_OFFSET: u32 = 0x18_6000;
pub const BANK1_BYTES: u32 = 0x15_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub image_len: u32,
    pub target_bank: u32,
    pub image_sha256: [u8; 32],
    pub build_id: [u8; 32],
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The version a manifest's header claims, unverified: `None` unless it has
/// a version 2 or retired version 1 manifest's length and magic (both keep
/// the version at byte 8). For a signer picking the next version above the
/// ones it already signed; a feeder only trusts `offer::admit_offer`.
pub fn claimed_version(bytes: &[u8]) -> Option<u32> {
    let v1 = bytes.len() == RETIRED_LEN && bytes[..8] == RETIRED_MAGIC;
    let v2 = bytes.len() == offer::MANIFEST_LEN && bytes[..8] == offer::MAGIC;
    (v1 || v2).then(|| u32_at(bytes, 8))
}

crate::named_enum! {
    /// The step of a sector write that failed (`PLOTA1 error step=`).
    pub enum SectorStep {
        Erase = "erase",
        Program = "program",
        Read = "read",
        /// A page read back different from what was programmed.
        Readback = "readback",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// A chunk outside the staged sector, already received, or of the wrong
    /// length.
    Order,
    /// The image does not start with the loader's marker.
    Marker,
    /// A programmed sector did not read back.
    Readback,
    /// The whole image's SHA-256 differs from the manifest.
    Hash,
    /// A call that does not fit the writer's state.
    State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Receiving,
    /// A full sector is staged; the port must erase, program and read it back.
    Programming,
    /// Every sector is written and the hash matched; only the marker remains.
    Ready,
    Failed(WriteError),
}

/// One sector to erase, program and read back, as a bank-1 offset.
#[derive(Debug, PartialEq, Eq)]
pub struct Program<'a> {
    pub offset: u32,
    pub data: &'a [u8],
}

/// Chunks per sector the received bitmap can track.
const MAX_CHUNKS: usize = u32::BITS as usize;

/// Writes one candidate into bank 1 with its marker withheld: the first
/// sector is programmed with erased marker bytes, and `commit` hands back the
/// real marker only after every sector read back and the hash matched.
///
/// A sector's chunks may arrive in any order, so the port can ask for all of
/// them at once; the sector is hashed in order once it is complete. Sectors
/// themselves are still written strictly in order.
pub struct Writer {
    manifest: Manifest,
    chunk: u32,
    /// The staged sector's received chunks, bit `i` for chunk `i`.
    have: u32,
    hasher: Sha256,
    sector: [u8; SECTOR],
    sector_offset: u32,
    phase: Phase,
}

impl Writer {
    /// `chunk` must divide the sector size into at most 32 chunks.
    pub fn new(manifest: Manifest, chunk: usize) -> Option<Self> {
        if chunk == 0 || !SECTOR.is_multiple_of(chunk) || SECTOR / chunk > MAX_CHUNKS {
            return None;
        }

        Some(Self {
            manifest,
            chunk: u32::try_from(chunk).ok()?,
            have: 0,
            hasher: Sha256::new(),
            sector: [0xff; SECTOR],
            sector_offset: 0,
            phase: Phase::Receiving,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn failed(&self) -> Option<WriteError> {
        match self.phase {
            Phase::Failed(error) => Some(error),
            _ => None,
        }
    }

    fn fail(&mut self, error: WriteError) -> WriteError {
        self.phase = Phase::Failed(error);
        error
    }

    /// The image bytes the staged sector holds: a whole sector, or the rest.
    fn sector_len(&self) -> u32 {
        self.manifest
            .image_len
            .saturating_sub(self.sector_offset)
            .min(SECTOR_BYTES)
    }

    /// The staged sector's chunks still to fetch, as `(offset, len)`; empty
    /// when nothing is wanted now.
    pub fn missing(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        let len = self.sector_len();
        let chunks = match self.phase {
            Phase::Receiving => len.div_ceil(self.chunk),
            _ => 0,
        };

        (0..chunks)
            .filter(|i| self.have & (1 << i) == 0)
            .map(move |i| {
                let at = i * self.chunk;
                (self.sector_offset + at, (len - at).min(self.chunk))
            })
    }

    /// Image bytes before the staged sector: written and read back.
    pub fn written(&self) -> u32 {
        self.sector_offset
    }

    /// Whether the chunk at `offset` is already in: in a sector written
    /// before, or received into the staged one. A port that takes chunks
    /// idempotently answers such a chunk without passing it on.
    pub fn has(&self, offset: u32) -> bool {
        if offset < self.sector_offset {
            return true;
        }
        let at = offset - self.sector_offset;
        let staged = matches!(self.phase, Phase::Receiving | Phase::Programming);
        staged
            && at < SECTOR_BYTES
            && at.is_multiple_of(self.chunk)
            && (self.phase == Phase::Programming || self.have & (1 << (at / self.chunk)) != 0)
    }

    /// The first chunk still to fetch, for a port that fetches one at a time.
    pub fn next_request(&self) -> Option<(u32, u32)> {
        self.missing().next()
    }

    /// Take the chunk at `offset`. Returns a sector to program when its last
    /// chunk arrives (or the image ends); the port must then call `programmed`.
    pub fn accept(&mut self, offset: u32, data: &[u8]) -> Result<Option<Program<'_>>, WriteError> {
        if self.phase != Phase::Receiving {
            return Err(self.fail(WriteError::State));
        }
        let wanted = self.missing().find(|&(at, _)| at == offset);
        if wanted.is_none_or(|(_, len)| data.len() != len as usize) {
            return Err(self.fail(WriteError::Order));
        }
        if offset == 0 && data.len() >= MARKER.len() && data[..MARKER.len()] != MARKER {
            return Err(self.fail(WriteError::Marker));
        }

        let at = offset - self.sector_offset;
        let start = at as usize;
        self.sector[start..start + data.len()].copy_from_slice(data);
        self.have |= 1 << (at / self.chunk);
        if self.missing().next().is_some() {
            return Ok(None);
        }

        let len = self.sector_len() as usize;
        self.hasher.update(&self.sector[..len]);
        self.phase = Phase::Programming;
        if self.sector_offset == 0 {
            // Withhold the marker: bank 1 must not be valid before commit.
            self.sector[..MARKER.len()].fill(0xff);
        }

        Ok(Some(Program {
            offset: BANK1_OFFSET + self.sector_offset,
            data: &self.sector[..len],
        }))
    }

    /// Report the staged sector's readback. A mismatch fails the update.
    pub fn programmed(&mut self, readback_ok: bool) -> Result<(), WriteError> {
        if self.phase != Phase::Programming {
            return Err(self.fail(WriteError::State));
        }
        if !readback_ok {
            return Err(self.fail(WriteError::Readback));
        }

        self.sector_offset += SECTOR_BYTES;
        self.have = 0;
        self.sector = [0xff; SECTOR];
        if self.sector_offset < self.manifest.image_len {
            self.phase = Phase::Receiving;
            return Ok(());
        }

        let digest: [u8; 32] = self.hasher.clone().finalize().into();
        if digest != self.manifest.image_sha256 {
            return Err(self.fail(WriteError::Hash));
        }
        self.phase = Phase::Ready;
        Ok(())
    }

    /// The marker write that activates bank 1, once and only once everything
    /// else is verified. The port programs it last and reads it back.
    pub fn commit(&mut self) -> Result<(u32, [u8; 8]), WriteError> {
        if self.phase != Phase::Ready {
            return Err(self.fail(WriteError::State));
        }
        Ok((BANK1_OFFSET, MARKER))
    }
}

/// What KM0 does with an unconfirmed bank-1 image early in each boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trial {
    /// Confirmed: boot normally.
    Run,
    /// Unconfirmed: store this count and boot.
    Count(u32),
    /// Unconfirmed `limit` times: clear bank 1's marker and reset, so the
    /// loader falls back to bank 0.
    GiveWay,
}

/// `counted`: unconfirmed boots already recorded for this image.
pub fn trial(confirmed: bool, counted: u32, limit: u32) -> Trial {
    if confirmed {
        Trial::Run
    } else if counted >= limit.max(1) {
        Trial::GiveWay
    } else {
        Trial::Count(counted + 1)
    }
}

/// A confirmation record names exactly one image by its SHA-256.
pub const CONFIRM_MAGIC: [u8; 4] = *b"PLCF";
pub const CONFIRM_LEN: usize = 4 + 32 + 4;

pub fn confirm_record(image_sha256: &[u8; 32], version: u32) -> [u8; CONFIRM_LEN] {
    let mut out = [0u8; CONFIRM_LEN];
    out[..4].copy_from_slice(&CONFIRM_MAGIC);
    out[4..36].copy_from_slice(image_sha256);
    out[36..].copy_from_slice(&version.to_le_bytes());
    out
}

/// Whether `record` confirms the image with this hash; returns its version.
pub fn confirmed(record: &[u8], image_sha256: &[u8; 32]) -> Option<u32> {
    (record.len() == CONFIRM_LEN && record[..4] == CONFIRM_MAGIC && record[4..36] == *image_sha256)
        .then(|| u32_at(record, 36))
}

/// The largest chunk the device API's update download asks for; it
/// divides a sector, so a sector is always whole chunks.
pub const MAX_CHUNK: u32 = 512;

const _: () = assert!(SECTOR.is_multiple_of(MAX_CHUNK as usize));
