//! Slot I/O and record encoding shared by the feed journal and settings store.
//!
//! Both keep 128-byte little-endian records in two alternating erase units,
//! checked by CRC-32/ISO-HDLC of bytes 0..120 at 120 and committed by the
//! `CMIT` marker at 124.

use core::ffi::{c_uint, c_void};
use core::ptr;

pub const RECORD_BYTES: usize = 128;
pub(crate) const COMMIT: [u8; 4] = *b"CMIT";

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
#[repr(C)]
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
    // SAFETY (all three): `FeedJournal::open`/`create` require the callbacks
    // to be sound for `context`; the buffers are live Rust slices.
    pub(crate) fn read(&self, slot: c_uint, offset: u32, bytes: &mut [u8]) -> bool {
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
        self.erase.is_some_and(|f| unsafe { f(self.context, slot) })
    }
}

pub(crate) fn put(p: &mut [u8], mut value: u64, count: usize) {
    for byte in &mut p[..count] {
        *byte = value as u8;
        value >>= 8;
    }
}

pub(crate) fn get(p: &[u8], count: usize) -> u64 {
    p[..count]
        .iter()
        .enumerate()
        .fold(0, |value, (i, &b)| value | (u64::from(b) << (i * 8)))
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
