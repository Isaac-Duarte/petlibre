//! Slot I/O and record encoding shared by the feed journal and settings store.
//!
//! Both keep 128-byte little-endian records in two alternating erase units,
//! checked by CRC-32/ISO-HDLC of bytes 0..120 at 120 and committed by the
//! `CMIT` marker at 124.

use core::ffi::{c_uint, c_void};
use core::ptr;

pub const RECORD_BYTES: usize = 128;
/// [`RECORD_BYTES`] as the header's length field stores it.
pub(crate) const RECORD_LEN: u16 = 128;
pub(crate) const COMMIT: [u8; 4] = *b"CMIT";

/// The header every store's record shares after its 8-byte magic: the format
/// version (u16) and the record length (u16).
pub(crate) const VERSION_AT: usize = 8;
pub(crate) const LENGTH_AT: usize = 10;
/// CRC-32 of bytes `..CRC_AT`, stored at `CRC_AT`.
pub(crate) const CRC_AT: usize = 120;
/// `COMMIT`, programmed last, after the rest of the record verified.
pub(crate) const COMMIT_AT: usize = 124;
/// [`COMMIT_AT`] as a slot offset.
pub(crate) const COMMIT_OFFSET: u32 = 124;
/// How much of a unit's erased tail a scan reads at once.
pub(crate) const TAIL_CHUNK: u32 = 64;

const _: () = assert!(COMMIT_AT == CRC_AT + 4 && RECORD_BYTES == COMMIT_AT + COMMIT.len());
const _: () = assert!(RECORD_LEN as usize == RECORD_BYTES && COMMIT_OFFSET as usize == COMMIT_AT);

pub type ReadFn = unsafe extern "C" fn(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool;

pub type ProgramFn = unsafe extern "C" fn(
    context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *const c_void,
    size: usize,
) -> bool;

pub type EraseFn = unsafe extern "C" fn(context: *mut c_void, slot: c_uint) -> bool;

/// Two EXCLUSIVE, nonoverlapping erase units, assigned only after a complete
/// physical flash map/backup. This interface contains no physical addresses.
/// Reads must bypass/invalidate stale caches; writes return only after durable
/// completion. Program permits aligned 4-byte granules, only 1->0, splitting
/// physical pages as needed. Erase affects exactly one whole unit. All methods
/// must be bounded. False may mean partial OR completed I/O; never retry
/// blindly. The backend must independently enforce output inactivity and SDK
/// flash/IRQ synchronization.
#[derive(Clone, Copy, Debug)]
pub struct JournalIo {
    pub context: *mut c_void,
    pub unit_bytes: u32,
    pub read: Option<ReadFn>,
    pub program: Option<ProgramFn>,
    pub erase: Option<EraseFn>,
}

impl Default for JournalIo {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            unit_bytes: 0,
            read: None,
            program: None,
            erase: None,
        }
    }
}

#[allow(unsafe_code)]
impl JournalIo {
    // In all three, `FeedJournal::open`/`create` require the callbacks to
    // be sound for `context`; the buffers are live Rust slices.
    pub(crate) fn read(&self, slot: c_uint, offset: u32, bytes: &mut [u8]) -> bool {
        // SAFETY: the callback contract above.
        self.read.is_some_and(|f| unsafe {
            f(
                self.context,
                slot,
                offset,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        })
    }

    pub(crate) fn program(&self, slot: c_uint, offset: u32, bytes: &[u8]) -> bool {
        // SAFETY: the callback contract above.
        self.program.is_some_and(|f| unsafe {
            f(
                self.context,
                slot,
                offset,
                bytes.as_ptr().cast(),
                bytes.len(),
            )
        })
    }

    pub(crate) fn erase(&self, slot: c_uint) -> bool {
        // SAFETY: the callback contract above.
        self.erase.is_some_and(|f| unsafe { f(self.context, slot) })
    }
}

// Little-endian record fields at the start of `p`.

pub(crate) fn put_u16(p: &mut [u8], value: u16) {
    p[..2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_u32(p: &mut [u8], value: u32) {
    p[..4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_u64(p: &mut [u8], value: u64) {
    p[..8].copy_from_slice(&value.to_le_bytes());
}

fn field<const N: usize>(p: &[u8]) -> [u8; N] {
    let mut bytes = [0; N];
    bytes.copy_from_slice(&p[..N]);
    bytes
}

pub(crate) fn get_u16(p: &[u8]) -> u16 {
    u16::from_le_bytes(field(p))
}

pub(crate) fn get_u32(p: &[u8]) -> u32 {
    u32::from_le_bytes(field(p))
}

pub(crate) fn get_u64(p: &[u8]) -> u64 {
    u64::from_le_bytes(field(p))
}

/// CRC-32/ISO-HDLC: reflected 0x04c11db7, init/xor-out all ones.
pub fn crc32(p: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for &byte in p {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xedb8_8320 } else { 0 };
        }
    }
    !value
}
