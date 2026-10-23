//! FW10/FW11 update logic (design: docs/firmware/ota-design.md): the signed
//! manifest, the bank-1 sector writer and the boot-trial decision.
//!
//! Sans-IO. The board port fetches chunks, runs each erase/program/readback
//! inside a flash lease and writes the marker last; this module only decides
//! what to write and whether the result may be activated.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

pub const MAGIC: [u8; 8] = *b"PLOTA1\0\0";
/// magic, version, image length, target bank (u32), image SHA-256, build ID.
pub const MANIFEST_BODY: usize = 8 + 4 + 4 + 4 + 32 + 32;
pub const MANIFEST_LEN: usize = MANIFEST_BODY + 64;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    Length,
    Magic,
    Signature,
    Bank,
    Size,
    Downgrade,
}

/// The signed part of a manifest, as `parse_manifest` reads it. The signer
/// (the owner's server tool) appends the 64-byte Ed25519 signature.
pub fn manifest_body(manifest: &Manifest) -> [u8; MANIFEST_BODY] {
    let mut out = [0u8; MANIFEST_BODY];
    out[..8].copy_from_slice(&MAGIC);
    out[8..12].copy_from_slice(&manifest.version.to_le_bytes());
    out[12..16].copy_from_slice(&manifest.image_len.to_le_bytes());
    out[16..20].copy_from_slice(&manifest.target_bank.to_le_bytes());
    out[20..52].copy_from_slice(&manifest.image_sha256);
    out[52..84].copy_from_slice(&manifest.build_id);
    out
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Accept a manifest only if it is signed by `key`, targets bank 1, fits
/// the reviewed bound, and is newer than `installed_version`.
pub fn parse_manifest(
    bytes: &[u8],
    key: &[u8; 32],
    installed_version: u32,
) -> Result<Manifest, ManifestError> {
    if bytes.len() != MANIFEST_LEN {
        return Err(ManifestError::Length);
    }
    if bytes[..8] != MAGIC {
        return Err(ManifestError::Magic);
    }

    let key = VerifyingKey::from_bytes(key).map_err(|_| ManifestError::Signature)?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&bytes[MANIFEST_BODY..]);
    key.verify_strict(&bytes[..MANIFEST_BODY], &Signature::from_bytes(&signature))
        .map_err(|_| ManifestError::Signature)?;

    let mut image_sha256 = [0u8; 32];
    image_sha256.copy_from_slice(&bytes[20..52]);
    let mut build_id = [0u8; 32];
    build_id.copy_from_slice(&bytes[52..84]);
    let manifest = Manifest {
        version: u32_at(bytes, 8),
        image_len: u32_at(bytes, 12),
        target_bank: u32_at(bytes, 16),
        image_sha256,
        build_id,
    };

    if manifest.target_bank != 1 {
        return Err(ManifestError::Bank);
    }
    if manifest.image_len < SECTOR_BYTES || manifest.image_len > BANK1_BYTES {
        return Err(ManifestError::Size);
    }
    if manifest.version <= installed_version {
        return Err(ManifestError::Downgrade);
    }

    Ok(manifest)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// A chunk out of order, of the wrong length, or after the end.
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

/// Writes one candidate into bank 1 with its marker withheld: the first
/// sector is programmed with erased marker bytes, and `commit` hands back the
/// real marker only after every sector read back and the hash matched.
pub struct Writer {
    manifest: Manifest,
    chunk: usize,
    received: u32,
    hasher: Sha256,
    sector: [u8; SECTOR],
    fill: usize,
    sector_offset: u32,
    phase: Phase,
}

impl Writer {
    /// `chunk` must divide the sector size.
    pub fn new(manifest: Manifest, chunk: usize) -> Option<Self> {
        (chunk != 0 && SECTOR.is_multiple_of(chunk)).then(|| Self {
            manifest,
            chunk,
            received: 0,
            hasher: Sha256::new(),
            sector: [0xff; SECTOR],
            fill: 0,
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

    /// The next chunk to fetch, or `None` when nothing is wanted now.
    pub fn next_request(&self) -> Option<(u32, u32)> {
        (self.phase == Phase::Receiving && self.received < self.manifest.image_len).then(|| {
            let left = self.manifest.image_len - self.received;
            let chunk = u32::try_from(self.chunk).unwrap_or(u32::MAX);
            (self.received, left.min(chunk))
        })
    }

    /// Take the chunk at `offset`. Returns a sector to program when one is
    /// complete (or the image ends); the port must then call `programmed`.
    pub fn accept(&mut self, offset: u32, data: &[u8]) -> Result<Option<Program<'_>>, WriteError> {
        let Some((want, len)) = self.next_request() else {
            return Err(self.fail(WriteError::State));
        };
        if offset != want || data.len() != len as usize {
            return Err(self.fail(WriteError::Order));
        }
        if offset == 0 && data.len() >= MARKER.len() && data[..MARKER.len()] != MARKER {
            return Err(self.fail(WriteError::Marker));
        }

        self.hasher.update(data);
        self.sector[self.fill..self.fill + data.len()].copy_from_slice(data);
        self.fill += data.len();
        self.received += len;
        if self.fill < SECTOR && self.received < self.manifest.image_len {
            return Ok(None);
        }

        self.phase = Phase::Programming;
        if self.sector_offset == 0 {
            // Withhold the marker: bank 1 must not be valid before commit.
            self.sector[..MARKER.len()].fill(0xff);
        }

        Ok(Some(Program {
            offset: BANK1_OFFSET + self.sector_offset,
            data: &self.sector[..self.fill],
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
        self.fill = 0;
        self.sector = [0xff; SECTOR];
        if self.received < self.manifest.image_len {
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

/// Delivery over the local transport: the updater pulls the manifest, then
/// the image in chunks, on `ota/get`; the server answers on `ota/data`.
/// 512 so a chunk answer fits the FW08 codec's existing packet bound with the
/// topic: no change to the running transport's buffers. Divides a sector.
pub const MAX_CHUNK: u32 = 512;
pub const GET_LEN: usize = 9;
pub const DATA_HEADER: usize = 10;
/// Room for `petlibre/1/<device id>/ota/data`.
pub const TOPIC_BUDGET: usize = 96;

// A chunk answer's publish: a header byte, up to four remaining-length bytes,
// the topic's length prefix, the topic, then the data header and chunk.
const _: () = assert!(
    5 + 2 + TOPIC_BUDGET + DATA_HEADER + MAX_CHUNK as usize <= crate::transport::MAX_PACKET
        && SECTOR.is_multiple_of(MAX_CHUNK as usize)
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Manifest,
    Chunk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// The server has no offer loaded.
    NoOffer,
    /// The requested range is outside the offered image or over `MAX_CHUNK`.
    Range,
}

/// `ota/get`: part, offset and length (both zero for the manifest).
pub fn encode_get(part: Part, offset: u32, len: u32) -> [u8; GET_LEN] {
    let mut out = [0u8; GET_LEN];
    out[0] = match part {
        Part::Manifest => 0,
        Part::Chunk => 1,
    };
    out[1..5].copy_from_slice(&offset.to_le_bytes());
    out[5..9].copy_from_slice(&len.to_le_bytes());
    out
}

pub fn decode_get(bytes: &[u8]) -> Option<(Part, u32, u32)> {
    if bytes.len() != GET_LEN {
        return None;
    }
    let part = match bytes[0] {
        0 => Part::Manifest,
        1 => Part::Chunk,
        _ => return None,
    };
    Some((part, u32_at(bytes, 1), u32_at(bytes, 5)))
}

/// The fixed header of an `ota/data` answer; the data bytes follow it.
pub fn encode_data_header(part: Part, status: Status, offset: u32, len: u32) -> [u8; DATA_HEADER] {
    let mut out = [0u8; DATA_HEADER];
    out[0] = matches!(part, Part::Chunk) as u8;
    out[1] = match status {
        Status::Ok => 0,
        Status::NoOffer => 1,
        Status::Range => 2,
    };
    out[2..6].copy_from_slice(&offset.to_le_bytes());
    out[6..10].copy_from_slice(&len.to_le_bytes());
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Data<'a> {
    pub part: Part,
    pub status: Status,
    pub offset: u32,
    pub bytes: &'a [u8],
}

/// Accept an `ota/data` answer only if its declared length matches.
pub fn decode_data(bytes: &[u8]) -> Option<Data<'_>> {
    if bytes.len() < DATA_HEADER {
        return None;
    }
    let part = match bytes[0] {
        0 => Part::Manifest,
        1 => Part::Chunk,
        _ => return None,
    };
    let status = match bytes[1] {
        0 => Status::Ok,
        1 => Status::NoOffer,
        2 => Status::Range,
        _ => return None,
    };

    let len = u32_at(bytes, 6) as usize;
    let body = &bytes[DATA_HEADER..];
    (body.len() == len && (status == Status::Ok || len == 0)).then_some(Data {
        part,
        status,
        offset: u32_at(bytes, 2),
        bytes: body,
    })
}
