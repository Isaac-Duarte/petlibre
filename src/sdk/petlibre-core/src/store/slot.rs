//! Slot I/O and record encoding shared by the feed journal and settings store.
//!
//! Both keep 128-byte little-endian records in two alternating erase units,
//! checked by CRC-32/ISO-HDLC of bytes 0..120 at 120 and committed by the
//! `CMIT` marker at 124.

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

/// Two or more EXCLUSIVE, nonoverlapping erase units, assigned only after a
/// complete physical flash map/backup. This interface contains no physical
/// addresses. Reads must bypass/invalidate stale caches; writes return only
/// after durable completion. Program permits aligned 4-byte granules, only
/// 1->0, splitting physical pages as needed. Erase affects exactly one whole
/// unit. All methods must be bounded. False may mean partial OR completed
/// I/O; never retry blindly. The backend must independently enforce output
/// inactivity and SDK flash/IRQ synchronization.
pub trait SlotIo {
    /// The size of one erase unit, in bytes.
    fn unit_bytes(&self) -> u32;
    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool;
    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool;
    fn erase(&self, slot: u32) -> bool;

    /// False for I/O that only reads (mapped flash with no lease): a store
    /// refuses its writes up front rather than failing part-way.
    fn writable(&self) -> bool {
        true
    }
}

/// A store's I/O once opened; before that, every call is refused.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bound<I>(Option<I>);

impl<I> Bound<I> {
    pub(crate) const NONE: Self = Self(None);

    pub(crate) fn new(io: I) -> Self {
        Self(Some(io))
    }
}

impl<I> Default for Bound<I> {
    fn default() -> Self {
        Self::NONE
    }
}

impl<I: SlotIo> Bound<I> {
    pub(crate) fn unit_bytes(&self) -> u32 {
        self.0.as_ref().map_or(0, SlotIo::unit_bytes)
    }

    pub(crate) fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.0
            .as_ref()
            .is_some_and(|io| io.read(slot, offset, bytes))
    }

    pub(crate) fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        self.0
            .as_ref()
            .is_some_and(|io| io.program(slot, offset, bytes))
    }

    pub(crate) fn erase(&self, slot: u32) -> bool {
        self.0.as_ref().is_some_and(|io| io.erase(slot))
    }

    pub(crate) fn writable(&self) -> bool {
        self.0.as_ref().is_some_and(SlotIo::writable)
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
