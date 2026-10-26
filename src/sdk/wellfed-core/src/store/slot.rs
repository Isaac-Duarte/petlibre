//! The dual-slot store every record on NOR but the event ring is kept in:
//! the SDK's feed journal, seed and update store here, and the product's
//! settings, Wi-Fi credentials and client table in
//! `wellfed_feeder::store`. Each store keeps one little-endian record in
//! each of two alternating erase units and supplies its own field layout
//! and recovery policy; [`DualSlot`] owns the header, the checks, the write
//! order and what a unit can hold after a cut.
//!
//! # The record
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..8 | the store's magic |
//! | 8..10 | format version (u16) |
//! | 10..12 | record length (u16) |
//! | 12..N-8 | the store's fields; unused bytes zero |
//! | N-8..N-4 | CRC-32/ISO-HDLC of bytes 0..N-8 |
//! | N-4..N | `CMIT`, programmed last |
//!
//! # The torn-write rule
//!
//! A save writes the unit that does not hold the newest record: it erases
//! that unit, checks the whole unit erased, programs everything but the
//! marker, checks it, programs the marker, and checks the whole record.
//! A record counts only once its marker is down and its checksum holds, so
//! a power cut at any point leaves the other unit untouched and this one in
//! one of the states [`Found`] names: erased, cut short ([`Found::Partial`]),
//! unmarked or torn by a cut erase, or the new record. Programming lays
//! bytes down in order; an erase raises bits in no particular order, so a
//! cut erase leaves anything at all, which fails the checksum or loses the
//! marker. The slot layer never retries a failed step; the store records
//! the fault, and the journal and the newest-wins stores write nothing more
//! until they are opened again (settings writes its next change to the same
//! unit, which the rule makes as safe as the first try).
//!
//! What a state means is the store's policy, since what may be lost
//! differs:
//!
//! - **Newest valid wins** ([`newest`]): the seed, Wi-Fi, client and update
//!   stores own their sectors outright, so anything but a valid record is a
//!   save of theirs cut short, and losing the newest change is safe. A
//!   newer format version anywhere makes the store read-only.
//! - **Settings** also falls back to the older record, reporting it
//!   recovered unless the other unit is erased. With no valid record it
//!   starts empty only after a cut first save of its own (`Partial` beside
//!   `Erased`); anything else is corrupt, and a newer format version
//!   read-only, until an explicit reset.
//! - **The feed journal** may never forget a reservation. Two valid records
//!   must be consecutive; one valid record stands beside an erased unit, or
//!   beside a cut write (`Partial`, `Unmarked` or `Torn`): a pending
//!   reservation is resolved again, and beside a record at rest the cut
//!   write is the next reservation, resolved as uncertain under the next
//!   ID. Anything else, including a newer format version, is corrupt.
//!
//! The journal and settings read the whole unit ([`Extent::Unit`]): written
//! bytes after the record make the unit invalid, so unrelated data is never
//! mistaken for an empty store. The others read the record alone.

use core::ffi::c_uint;

/// The record most stores keep, and where its checksum sits.
pub const RECORD_BYTES: usize = 128;
/// CRC-32 of bytes `..CRC_AT` in a [`RECORD_BYTES`] record.
pub const CRC_AT: usize = 120;

const COMMIT: [u8; 4] = *b"CMIT";
/// The header every record shares after its 8-byte magic: the format
/// version (u16) and the record length (u16).
const VERSION_AT: usize = 8;
const LENGTH_AT: usize = 10;
/// Where the store's fields start.
const FIELDS_AT: usize = 12;
/// How much of a unit's erased tail a scan reads at once.
const TAIL_CHUNK: u32 = 64;

const MARKER_BYTES: usize = COMMIT.len();
/// The checksum and the marker at a record's end.
const TRAILER_BYTES: usize = 4 + MARKER_BYTES;

const _: () = assert!(RECORD_BYTES == CRC_AT + TRAILER_BYTES);

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

/// How much of a unit a scan reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// The record alone.
    Record,
    /// The record and every byte after it, which must be erased.
    Unit,
}

/// What a store's record shares with every other's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub magic: [u8; 8],
    /// The format version this build writes and decodes. A higher one is
    /// [`Found::Future`]; a lower one is invalid, as no migration exists.
    pub version: u16,
    pub extent: Extent,
}

impl Layout {
    /// A new record: zero, with the header filled in.
    pub fn record<const N: usize>(&self) -> [u8; N] {
        const { assert!(N >= FIELDS_AT + TRAILER_BYTES && N <= 0xffff) };
        let mut p = [0u8; N];
        if let Some(magic) = p.first_chunk_mut::<8>() {
            *magic = self.magic;
        }
        put_u16::<VERSION_AT, N>(&mut p, self.version);
        put_u16::<LENGTH_AT, N>(&mut p, u16::try_from(N).unwrap_or(u16::MAX));
        p
    }
}

/// Fill in the checksum and the marker of a finished record.
pub fn seal<const N: usize>(p: &mut [u8; N]) {
    let Some((body, trailer)) = p.split_last_chunk_mut::<TRAILER_BYTES>() else {
        return;
    };
    let crc = crc32(body);
    trailer[..4].copy_from_slice(&crc.to_le_bytes());
    trailer[4..].copy_from_slice(&COMMIT);
}

/// What one unit holds, by the torn-write rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found<V> {
    /// Every byte erased: never written, or an erase that finished.
    Erased,
    /// Committed, checked and decoded.
    Valid(V),
    /// Committed and checked, in a newer format version than this build's.
    Future,
    /// No marker, and the bytes are this store's own record cut short:
    /// some of the magic and then erased bytes, or the whole magic.
    Partial,
    /// No marker otherwise: an erase cut short over a record, or bytes
    /// another writer left.
    Unmarked,
    /// The marker, but a failing checksum: an erase cut short before it
    /// raised the marker's bits, or a record that decayed.
    Torn,
    /// Checked but not this store's record, or written bytes after the
    /// record in a unit read whole.
    Invalid,
}

impl<V> Found<V> {
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid(_))
    }

    pub const fn is_erased(&self) -> bool {
        matches!(self, Self::Erased)
    }

    pub const fn is_future(&self) -> bool {
        matches!(self, Self::Future)
    }
}

/// Which of two units a store opens on, by the newest-valid-wins rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick<V> {
    /// A newer format version is present: the store is read-only.
    Future,
    /// The newer valid record, ties to unit 0, and its unit.
    Newest(c_uint, V),
    /// No valid record in either unit.
    Nothing,
}

/// The newest valid record of `a` (unit 0) and `b` (unit 1) by
/// `generation`, unless either holds a newer format version.
pub fn newest<V>(a: Found<V>, b: Found<V>, generation: impl Fn(&V) -> u64) -> Pick<V> {
    if a.is_future() || b.is_future() {
        return Pick::Future;
    }
    match (a, b) {
        (Found::Valid(x), Found::Valid(y)) => {
            if generation(&x) >= generation(&y) {
                Pick::Newest(0, x)
            } else {
                Pick::Newest(1, y)
            }
        }
        (Found::Valid(x), _) => Pick::Newest(0, x),
        (_, Found::Valid(y)) => Pick::Newest(1, y),
        _ => Pick::Nothing,
    }
}

/// The unit a save writes: the first after an empty start, then the one
/// not holding the newest record.
pub const fn target(generation: u64, slot: c_uint) -> c_uint {
    if generation == 0 { 0 } else { 1 - slot }
}

/// Whether a save erases the unit first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prepare {
    Erase,
    /// The unit was just checked erased, by a first provisioning.
    Erased,
}

/// A store's two units and its record layout; before [`DualSlot::bind`],
/// every I/O call is refused.
#[derive(Clone, Copy, Debug)]
pub struct DualSlot<I> {
    io: Option<I>,
    layout: &'static Layout,
}

impl<I> DualSlot<I> {
    pub const fn unbound(layout: &'static Layout) -> Self {
        Self { io: None, layout }
    }

    pub const fn new(io: I, layout: &'static Layout) -> Self {
        Self {
            io: Some(io),
            layout,
        }
    }

    pub fn bind(&mut self, io: I) {
        self.io = Some(io);
    }
}

impl<I: SlotIo> DualSlot<I> {
    pub fn unit_bytes(&self) -> u32 {
        self.io.as_ref().map_or(0, SlotIo::unit_bytes)
    }

    /// Whether the I/O can write at all.
    pub fn writable(&self) -> bool {
        self.io.as_ref().is_some_and(SlotIo::writable)
    }

    fn read(&self, slot: c_uint, offset: u32, bytes: &mut [u8]) -> bool {
        self.io
            .as_ref()
            .is_some_and(|io| io.read(slot, offset, bytes))
    }

    fn program(&self, slot: c_uint, offset: u32, bytes: &[u8]) -> bool {
        self.io
            .as_ref()
            .is_some_and(|io| io.program(slot, offset, bytes))
    }

    fn erase(&self, slot: c_uint) -> bool {
        self.io.as_ref().is_some_and(|io| io.erase(slot))
    }

    /// Read unit `slot` by the torn-write rule, `decode` taking a record
    /// whose header, version and checksum hold. `None` when a read fails.
    pub fn scan<const N: usize, V>(
        &self,
        slot: c_uint,
        decode: impl FnOnce(&[u8; N]) -> Option<V>,
    ) -> Option<Found<V>> {
        let mut p = [0u8; N];
        Some(match self.classify(slot, &mut p)? {
            Class::Current => decode(&p).map_or(Found::Invalid, Found::Valid),
            Class::Erased => Found::Erased,
            Class::Future => Found::Future,
            Class::Partial => Found::Partial,
            Class::Unmarked => Found::Unmarked,
            Class::Torn => Found::Torn,
            Class::Invalid => Found::Invalid,
        })
    }

    /// The record's state, before its fields are decoded.
    fn classify(&self, slot: c_uint, p: &mut [u8]) -> Option<Class> {
        if !self.read(slot, 0, p) {
            return None;
        }
        let tail_erased = match self.layout.extent {
            Extent::Record => true,
            Extent::Unit => self.tail_erased(slot, p.len())?,
        };

        if !tail_erased {
            return Some(Class::Invalid);
        }
        if erased(p) {
            return Some(Class::Erased);
        }
        let Some((body, trailer)) = p.split_last_chunk::<TRAILER_BYTES>() else {
            return Some(Class::Invalid);
        };
        if trailer[4..] != COMMIT {
            return Some(if ours(p, &self.layout.magic) {
                Class::Partial
            } else {
                Class::Unmarked
            });
        }
        if trailer[..4] != crc32(body).to_le_bytes() {
            return Some(Class::Torn);
        }

        let header = p.first_chunk::<FIELDS_AT>()?;
        let version = u16::from_le_bytes([header[VERSION_AT], header[VERSION_AT + 1]]);
        let length = u16::from_le_bytes([header[LENGTH_AT], header[LENGTH_AT + 1]]);
        Some(
            if header[..8] != self.layout.magic[..] || usize::from(length) != p.len() {
                Class::Invalid
            } else if version > self.layout.version {
                Class::Future
            } else if version != self.layout.version {
                Class::Invalid
            } else {
                Class::Current
            },
        )
    }

    /// Whether every byte of unit `slot` from `from` on is erased; `None`
    /// when a read fails. Every chunk is read, so a failing read is
    /// reported even past written bytes.
    fn tail_erased(&self, slot: c_uint, from: usize) -> Option<bool> {
        let unit = self.unit_bytes();
        let mut offset = u32::try_from(from).ok()?;
        let mut chunk = [0u8; TAIL_CHUNK as usize];
        let mut all = true;
        while offset < unit {
            let size = (unit - offset).min(TAIL_CHUNK) as usize;
            let part = chunk.get_mut(..size)?;
            if !self.read(slot, offset, part) {
                return None;
            }
            all &= erased(part);
            offset = offset.saturating_add(TAIL_CHUNK);
        }
        Some(all)
    }

    /// Whether unit `slot` is erased throughout, read into `p` and after.
    fn unit_erased(&self, slot: c_uint, p: &mut [u8]) -> bool {
        self.read(slot, 0, p) && erased(p) && self.tail_erased(slot, p.len()) == Some(true)
    }

    /// Write `record` to unit `slot` by the torn-write rule. False when any
    /// step failed; the unit may then hold any state a cut leaves.
    pub fn write<const N: usize>(&self, slot: c_uint, record: &[u8; N], prepare: Prepare) -> bool {
        let mut checked = [0u8; N];
        let Some((body, marker)) = record.split_last_chunk::<MARKER_BYTES>() else {
            return false;
        };
        let Ok(marker_at) = u32::try_from(body.len()) else {
            return false;
        };
        (prepare == Prepare::Erased || self.erase(slot))
            && self.unit_erased(slot, &mut checked)
            && self.program(slot, 0, body)
            && self.read(slot, 0, &mut checked)
            && checked.split_last_chunk::<MARKER_BYTES>() == Some((body, &[u8::MAX; MARKER_BYTES]))
            && self.program(slot, marker_at, marker)
            && self.read(slot, 0, &mut checked)
            && checked == *record
    }

    /// Erase both units and check them erased: a store's explicit reset.
    pub fn clear(&self) -> bool {
        let mut p = [0u8; TAIL_CHUNK as usize];
        self.erase(0) && self.erase(1) && self.unit_erased(0, &mut p) && self.unit_erased(1, &mut p)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Erased,
    /// Header, version and checksum hold: the store decodes the fields.
    Current,
    Future,
    Partial,
    Unmarked,
    Torn,
    Invalid,
}

fn erased(p: &[u8]) -> bool {
    p.iter().all(|&b| b == u8::MAX)
}

/// Programming lays bytes down in order, so a save cut short leaves a prefix
/// of the magic followed by erased bytes, or the whole magic. Anything else
/// is not evidence of the store's own write.
fn ours(p: &[u8], magic: &[u8; 8]) -> bool {
    let matched = magic.iter().zip(p).take_while(|(m, r)| m == r).count();
    matched == magic.len() || p.get(matched..).is_some_and(erased)
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
    let mut crc = Crc32::new();
    crc.update(p);
    crc.finish()
}

/// [`crc32`] over bytes that arrive in pieces, such as a record read from
/// flash a chunk at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub const fn new() -> Self {
        Self(u32::MAX)
    }

    pub fn update(&mut self, p: &[u8]) {
        for &byte in p {
            self.0 ^= u32::from(byte);
            for _ in 0..8 {
                self.0 = (self.0 >> 1) ^ if self.0 & 1 != 0 { 0xedb8_8320 } else { 0 };
            }
        }
    }

    pub const fn finish(self) -> u32 {
        !self.0
    }
}
