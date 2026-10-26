//! Slot I/O and record encoding shared by every dual-slot store, the SDK's
//! here and the product's in `wellfed_feeder::store`.
//!
//! Each keeps 128-byte little-endian records in two alternating erase units,
//! checked by CRC-32/ISO-HDLC of bytes 0..120 at 120 and committed by the
//! `CMIT` marker at 124.

pub const RECORD_BYTES: usize = 128;
/// [`RECORD_BYTES`] as the header's length field stores it.
pub const RECORD_LEN: u16 = 128;
pub const COMMIT: [u8; 4] = *b"CMIT";

/// The header every store's record shares after its 8-byte magic: the format
/// version (u16) and the record length (u16).
pub const VERSION_AT: usize = 8;
pub const LENGTH_AT: usize = 10;
/// CRC-32 of bytes `..CRC_AT`, stored at `CRC_AT`.
pub const CRC_AT: usize = 120;
/// `COMMIT`, programmed last, after the rest of the record verified.
pub const COMMIT_AT: usize = 124;
/// [`COMMIT_AT`] as a slot offset.
pub const COMMIT_OFFSET: u32 = 124;
/// How much of a unit's erased tail a scan reads at once.
pub const TAIL_CHUNK: u32 = 64;

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
pub struct Bound<I>(Option<I>);

impl<I> Bound<I> {
    pub const NONE: Self = Self(None);

    pub fn new(io: I) -> Self {
        Self(Some(io))
    }
}

impl<I> Default for Bound<I> {
    fn default() -> Self {
        Self::NONE
    }
}

impl<I: SlotIo> Bound<I> {
    pub fn unit_bytes(&self) -> u32 {
        self.0.as_ref().map_or(0, SlotIo::unit_bytes)
    }

    pub fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.0
            .as_ref()
            .is_some_and(|io| io.read(slot, offset, bytes))
    }

    pub fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        self.0
            .as_ref()
            .is_some_and(|io| io.program(slot, offset, bytes))
    }

    pub fn erase(&self, slot: u32) -> bool {
        self.0.as_ref().is_some_and(|io| io.erase(slot))
    }

    pub fn writable(&self) -> bool {
        self.0.as_ref().is_some_and(SlotIo::writable)
    }
}

// Little-endian record fields at a fixed offset `AT` in a fixed-size record.
// An offset past the record's end fails to compile, so no input can make
// these panic.

const fn fits<const AT: usize, const N: usize, const L: usize>() {
    assert!(AT + N <= L, "record field past the record's end");
}

fn put<const AT: usize, const N: usize, const L: usize>(p: &mut [u8; L], bytes: [u8; N]) {
    const { fits::<AT, N, L>() };
    if let Some(field) = p.get_mut(AT..).and_then(<[u8]>::first_chunk_mut) {
        *field = bytes;
    }
}

fn get<const AT: usize, const N: usize, const L: usize>(p: &[u8; L]) -> [u8; N] {
    const { fits::<AT, N, L>() };
    p.get(AT..)
        .and_then(<[u8]>::first_chunk)
        .copied()
        .unwrap_or([0; N])
}

pub fn put_u16<const AT: usize, const L: usize>(p: &mut [u8; L], value: u16) {
    put::<AT, 2, L>(p, value.to_le_bytes());
}

pub fn put_u32<const AT: usize, const L: usize>(p: &mut [u8; L], value: u32) {
    put::<AT, 4, L>(p, value.to_le_bytes());
}

pub fn put_u64<const AT: usize, const L: usize>(p: &mut [u8; L], value: u64) {
    put::<AT, 8, L>(p, value.to_le_bytes());
}

pub fn get_u16<const AT: usize, const L: usize>(p: &[u8; L]) -> u16 {
    u16::from_le_bytes(get::<AT, 2, L>(p))
}

pub fn get_u32<const AT: usize, const L: usize>(p: &[u8; L]) -> u32 {
    u32::from_le_bytes(get::<AT, 4, L>(p))
}

pub fn get_u64<const AT: usize, const L: usize>(p: &[u8; L]) -> u64 {
    u64::from_le_bytes(get::<AT, 8, L>(p))
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
