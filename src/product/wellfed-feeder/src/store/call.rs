//! The meal call on NOR (storage sectors 40-47): one clip the owner
//! uploads, as `wellfed_core::clip` describes it.
//!
//! The first [`clip::DATA_AT`] bytes of sector 40 hold a 24-byte header,
//! little-endian; the stream follows, across the remaining sectors:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..8 | magic `PLFCAL01` |
//! | 8..10 | version |
//! | 10..12 | zero |
//! | 12..16 | the stream's bytes, 1 to [`clip::BYTES_MAX`] |
//! | 16..20 | the stream's CRC-32/ISO-HDLC |
//! | 20..24 | CRC-32/ISO-HDLC of bytes 0..20 |
//!
//! **One copy, header last.** An upload erases sector 40 first, header
//! and all, so the old clip is gone the moment a new one starts. Each
//! chunk erases its sector as it arrives at that sector's first byte, and
//! the header is programmed only once the whole stream reads back against
//! its CRC. A cut anywhere before that leaves no header, so no clip; a cut
//! while the header is programmed leaves one that fails its own CRC. A
//! clip, once stored, is read back against its CRC each boot too, so a
//! sector that lost bits plays nothing rather than noise.
//!
//! An upload in progress lives in RAM: a restart forgets it, and the
//! client starts again.

use core::ffi::c_uint;

use wellfed_core::clip;
use wellfed_core::store::slot::{Crc32, SlotIo, crc32, get_u32};
use wellfed_device_api::mailbox::{CallClip, CallView, chunk_wanted};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFCAL01";
const HEADER_BYTES: usize = 24;
const BYTES_AT: usize = 12;
const CRC_AT: usize = 16;
const HEADER_CRC_AT: usize = 20;
/// Bytes read back at a time while checking a stream.
const READ_BYTES: u32 = 64;

const _: () = assert!(HEADER_BYTES <= clip::DATA_AT as usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallStatus {
    Unopened,
    /// Writable: a clip or none, or an upload under way.
    Ready,
    /// A read or write failed: no clip, and nothing written this boot.
    Io,
}

impl CallStatus {
    /// The status's record word.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unopened => "unopened",
            Self::Ready => "ready",
            Self::Io => "io",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CallStore<I> {
    io: Option<I>,
    pub status: CallStatus,
    pub clip: CallClip,
}

impl<I> Default for CallStore<I> {
    fn default() -> Self {
        Self {
            io: None,
            status: CallStatus::Unopened,
            clip: CallClip::None,
        }
    }
}

fn header(bytes: u32, crc: u32) -> [u8; HEADER_BYTES] {
    let mut p = [0; HEADER_BYTES];
    p[..8].copy_from_slice(&MAGIC);
    p[8..10].copy_from_slice(&VERSION.to_le_bytes());
    p[BYTES_AT..BYTES_AT + 4].copy_from_slice(&bytes.to_le_bytes());
    p[CRC_AT..CRC_AT + 4].copy_from_slice(&crc.to_le_bytes());
    let check = crc32(&p[..HEADER_CRC_AT]);
    p[HEADER_CRC_AT..].copy_from_slice(&check.to_le_bytes());
    p
}

/// The stream's length and CRC from a header whose fields and own CRC
/// hold.
fn decode(p: &[u8; HEADER_BYTES]) -> Option<(u32, u32)> {
    let bytes = get_u32::<BYTES_AT, _>(p);
    let crc = get_u32::<CRC_AT, _>(p);
    let valid = p[..8] == MAGIC
        && p[8..10] == VERSION.to_le_bytes()
        && p[10..12] == [0, 0]
        && (1..=clip::BYTES_MAX).contains(&bytes)
        && get_u32::<HEADER_CRC_AT, _>(p) == crc32(&p[..HEADER_CRC_AT]);
    valid.then_some((bytes, crc))
}

/// The unit and offset holding stream byte `at`.
const fn place(at: u32) -> (c_uint, u32) {
    let at = clip::DATA_AT + at;
    (at / clip::SECTOR_BYTES, at % clip::SECTOR_BYTES)
}

impl<I: SlotIo> CallStore<I> {
    /// Bind to the call's eight erase units and load the clip, if one
    /// is stored whole.
    pub fn open(&mut self, io: I) -> CallStatus {
        let unit = io.unit_bytes();
        *self = Self {
            io: Some(io),
            ..Self::default()
        };
        if unit != clip::SECTOR_BYTES {
            self.status = CallStatus::Io;
            return self.status;
        }
        let mut p = [0; HEADER_BYTES];
        if !self.read(0, 0, &mut p) {
            self.status = CallStatus::Io;
            return self.status;
        }

        self.status = CallStatus::Ready;
        if let Some((bytes, crc)) = decode(&p)
            && self.stream_crc(bytes) == Some(crc)
        {
            self.clip = CallClip::Stored { bytes, crc };
        }
        self.status
    }

    pub fn writable(&self) -> bool {
        self.status == CallStatus::Ready && self.io.as_ref().is_some_and(SlotIo::writable)
    }

    /// The view the writer publishes; writable only once it is `armed`.
    pub fn view(&self, armed: bool) -> CallView {
        CallView {
            writable: armed && self.writable(),
            clip: self.clip,
        }
    }

    /// The stored clip's stream bytes, if one is stored: what the speaker
    /// may play.
    pub fn stored(&self) -> Option<u32> {
        match self.clip {
            CallClip::Stored { bytes, .. } => Some(bytes),
            CallClip::None | CallClip::Receiving { .. } => None,
        }
    }

    /// Erase the clip and wait for an upload of `bytes` with CRC `crc`.
    pub fn start(&mut self, bytes: u32, crc: u32) -> bool {
        if !(1..=clip::BYTES_MAX).contains(&bytes) || !self.erase(0) {
            return false;
        }
        self.clip = CallClip::Receiving {
            bytes,
            crc,
            received: 0,
        };
        true
    }

    /// Erase the clip.
    pub fn delete(&mut self) -> bool {
        if !self.erase(0) {
            return false;
        }
        self.clip = CallClip::None;
        true
    }

    /// Write the upload's next chunk, `data` at `offset`: true once it is
    /// written (or was already). The last chunk ends the upload: the
    /// stream is read back against its CRC and the header written, or the
    /// store keeps no clip.
    pub fn chunk(&mut self, offset: u32, data: &[u8]) -> bool {
        let CallClip::Receiving { bytes, crc, .. } = self.clip else {
            return false;
        };
        match chunk_wanted(self.clip, offset, data.len()) {
            Ok(false) => return true,
            Ok(true) => {}
            Err(_) => return false,
        }

        let (unit, at) = place(offset);
        // Sector 40 was erased when the upload started; each later sector
        // as its first chunk arrives.
        if unit != 0 && at == 0 && !self.erase(unit) {
            return false;
        }
        if !self.program_padded(unit, at, data) {
            return false;
        }
        let Some(received) = u32::try_from(data.len())
            .ok()
            .and_then(|len| offset.checked_add(len))
        else {
            return false;
        };
        self.clip = CallClip::Receiving {
            bytes,
            crc,
            received,
        };
        if received < bytes {
            return true;
        }

        if self.stream_crc(bytes) != Some(crc) {
            // The client's stream, or our writes, are not what it named:
            // keep no clip. The header was never written.
            self.clip = CallClip::None;
            return true;
        }
        if !self.program(0, 0, &header(bytes, crc)) {
            return false;
        }
        self.clip = CallClip::Stored { bytes, crc };
        true
    }

    /// Program `data`, whose length need not be whole words as the flash
    /// writes them: the last word is padded with erased bytes, which
    /// program nothing.
    fn program_padded(&mut self, unit: c_uint, offset: u32, data: &[u8]) -> bool {
        let (words, tail) = data.split_at(data.len() - data.len() % 4);
        if !words.is_empty() && !self.program(unit, offset, words) {
            return false;
        }
        if tail.is_empty() {
            return true;
        }
        let mut last = [u8::MAX; 4];
        last[..tail.len()].copy_from_slice(tail);
        let Some(at) = u32::try_from(words.len())
            .ok()
            .and_then(|len| offset.checked_add(len))
        else {
            return false;
        };
        self.program(unit, at, &last)
    }

    /// The CRC of the first `bytes` stream bytes as the flash holds them.
    fn stream_crc(&self, bytes: u32) -> Option<u32> {
        let mut crc = Crc32::new();
        let mut buffer = [0; READ_BYTES as usize];
        let mut at = 0;
        while at < bytes {
            let (unit, offset) = place(at);
            // Never across a sector: the stream starts and the sectors end
            // on multiples of the buffer.
            let len = (bytes - at).min(READ_BYTES);
            let chunk = buffer.get_mut(..len as usize)?;
            if !self.read(unit, offset, chunk) {
                return None;
            }
            crc.update(chunk);
            at += len;
        }
        Some(crc.finish())
    }

    fn read(&self, unit: c_uint, offset: u32, bytes: &mut [u8]) -> bool {
        self.io
            .as_ref()
            .is_some_and(|io| io.read(unit, offset, bytes))
    }

    /// A write that fails leaves no clip, and the store refuses every
    /// later one this boot.
    fn erase(&mut self, unit: c_uint) -> bool {
        let ok = self.writable() && self.io.as_ref().is_some_and(|io| io.erase(unit));
        self.failed(ok)
    }

    fn program(&mut self, unit: c_uint, offset: u32, bytes: &[u8]) -> bool {
        let ok = self.writable()
            && self
                .io
                .as_ref()
                .is_some_and(|io| io.program(unit, offset, bytes));
        self.failed(ok)
    }

    fn failed(&mut self, ok: bool) -> bool {
        if !ok {
            self.clip = CallClip::None;
            self.status = CallStatus::Io;
        }
        ok
    }
}

const _: () = assert!(
    clip::DATA_AT.is_multiple_of(READ_BYTES) && clip::SECTOR_BYTES.is_multiple_of(READ_BYTES)
);
