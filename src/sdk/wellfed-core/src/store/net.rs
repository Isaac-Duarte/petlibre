//! FW25: Wi-Fi credentials stored on NOR, set without a rebuild.
//!
//! Two alternating 128-byte records on the same slot I/O as the settings
//! store: magic `PLFNET01`, version, record size, generation, SSID length,
//! passphrase length, SSID (32 bytes), passphrase (64 bytes), zero padding,
//! CRC-32/ISO-HDLC of bytes 0..120 at 120 and the `CMIT` commit marker at 124.
//! A record with an empty SSID means "use the build's credentials", so a
//! clear keeps the generation rising. The newest valid record wins; a torn or
//! invalid newer slot falls back to the older one. The two sectors belong to
//! this store alone, so an unreadable record is a save of ours cut short and
//! may be overwritten. Only a newer schema version makes the store read-only;
//! then, as with no record, the build's credentials apply.

use core::ffi::c_uint;

use crate::store::slot::{
    COMMIT, COMMIT_AT, COMMIT_OFFSET, CRC_AT, JournalIo, LENGTH_AT, RECORD_BYTES, RECORD_LEN,
    VERSION_AT, crc32, get_u16, get_u32, get_u64, put_u16, put_u32, put_u64,
};

pub const VERSION: u16 = 1;
const MAGIC: [u8; 8] = *b"PLFNET01";
pub const MAX_SSID: usize = 32;
/// WPA2 passphrases are 8-63 printable ASCII characters.
pub const MIN_PSK: usize = 8;
pub const MAX_PSK: usize = 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Credentials {
    ssid: [u8; MAX_SSID],
    ssid_len: u8,
    psk: [u8; 64],
    psk_len: u8,
}

impl Credentials {
    /// A 1-32 byte SSID and an 8-63 character printable-ASCII passphrase.
    pub fn new(ssid: &[u8], psk: &[u8]) -> Option<Self> {
        if ssid.is_empty()
            || ssid.len() > MAX_SSID
            || !(MIN_PSK..=MAX_PSK).contains(&psk.len())
            || !psk.iter().all(|b| (0x20..0x7f).contains(b))
        {
            return None;
        }
        let mut out = Self {
            ssid: [0; MAX_SSID],
            ssid_len: u8::try_from(ssid.len()).ok()?,
            psk: [0; 64],
            psk_len: u8::try_from(psk.len()).ok()?,
        };
        out.ssid[..ssid.len()].copy_from_slice(ssid);
        out.psk[..psk.len()].copy_from_slice(psk);
        Some(out)
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }

    pub fn psk(&self) -> &[u8] {
        &self.psk[..usize::from(self.psk_len)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetStatus {
    Unopened,
    /// No record yet: the build's credentials, writable.
    Empty,
    /// A valid record (stored credentials, or a clear).
    Loaded,
    /// A newer schema version: read-only.
    ReadOnly,
    Io,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Erased,
    Valid(u64, Option<Credentials>),
    Future,
    Invalid,
}

#[derive(Clone, Copy, Debug)]
pub struct NetStore {
    io: JournalIo,
    pub status: NetStatus,
    pub generation: u64,
    pub slot: c_uint,
    /// `None` means the build's credentials.
    pub credentials: Option<Credentials>,
}

impl Default for NetStore {
    fn default() -> Self {
        Self {
            io: JournalIo::default(),
            status: NetStatus::Unopened,
            generation: 0,
            slot: 0,
            credentials: None,
        }
    }
}

fn encode(generation: u64, credentials: Option<&Credentials>) -> [u8; RECORD_BYTES] {
    let mut p = [0u8; RECORD_BYTES];
    p[..8].copy_from_slice(&MAGIC);
    put_u16(&mut p[VERSION_AT..], VERSION);
    put_u16(&mut p[LENGTH_AT..], RECORD_LEN);
    put_u64(&mut p[12..], generation);
    if let Some(c) = credentials {
        p[20] = c.ssid_len;
        p[21] = c.psk_len;
        p[22..22 + MAX_SSID].copy_from_slice(&c.ssid);
        p[54..54 + 64].copy_from_slice(&c.psk);
    }
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
    let (ssid_len, psk_len) = (usize::from(p[20]), usize::from(p[21]));
    if version != VERSION || generation == 0 || p[118..CRC_AT] != [0, 0] {
        return Scan::Invalid;
    }
    if ssid_len == 0 {
        return if psk_len == 0 && p[22..118].iter().all(|&b| b == 0) {
            Scan::Valid(generation, None)
        } else {
            Scan::Invalid
        };
    }
    // Unused bytes must be zero, so one encoding exists per value.
    if ssid_len > MAX_SSID
        || psk_len > MAX_PSK
        || p[22 + ssid_len..54].iter().any(|&b| b != 0)
        || p[54 + psk_len..118].iter().any(|&b| b != 0)
    {
        return Scan::Invalid;
    }
    match Credentials::new(&p[22..22 + ssid_len], &p[54..54 + psk_len]) {
        Some(c) => Scan::Valid(generation, Some(c)),
        None => Scan::Invalid,
    }
}

impl NetStore {
    fn scan(&self, slot: c_uint) -> Option<Scan> {
        let mut record = [0u8; RECORD_BYTES];
        self.io.read(slot, 0, &mut record).then(|| decode(&record))
    }

    /// Bind to two exclusive erase units and load the newest valid record.
    ///
    /// # Safety
    /// The I/O callbacks must be sound for `io.context` while the store is used.
    #[allow(unsafe_code)]
    pub unsafe fn open(&mut self, io: JournalIo) -> NetStatus {
        *self = Self {
            io,
            ..Self::default()
        };
        if io.read.is_none() || (io.unit_bytes as usize) < RECORD_BYTES {
            self.status = NetStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.scan(0), self.scan(1)) else {
            self.status = NetStatus::Io;
            return self.status;
        };

        let pick = match (a, b) {
            (Scan::Valid(ga, ca), Scan::Valid(gb, cb)) => {
                Some(if ga >= gb { (0, ga, ca) } else { (1, gb, cb) })
            }
            (Scan::Valid(g, c), _) => Some((0, g, c)),
            (_, Scan::Valid(g, c)) => Some((1, g, c)),
            _ => None,
        };
        self.status = if a == Scan::Future || b == Scan::Future {
            NetStatus::ReadOnly
        } else if let Some((slot, generation, credentials)) = pick {
            self.slot = slot;
            self.generation = generation;
            self.credentials = credentials;
            NetStatus::Loaded
        } else {
            // The two sectors are this store's alone, so anything else there
            // is a save of ours cut short: no credentials, still writable.
            NetStatus::Empty
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, NetStatus::Empty | NetStatus::Loaded)
    }

    /// Store `credentials`, or `None` to return to the build's; to the other
    /// slot with erase, program, verify, commit, verify. A failure keeps the
    /// previous credentials in memory.
    pub fn save(&mut self, credentials: Option<&Credentials>) -> bool {
        if !self.writable() || self.io.program.is_none() || self.io.erase.is_none() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = if self.generation == 0 {
            0
        } else {
            1 - self.slot
        };
        let bytes = encode(generation, credentials);
        let mut checked = [0u8; RECORD_BYTES];
        let ok = self.io.erase(slot)
            && self.io.read(slot, 0, &mut checked)
            && checked.iter().all(|&b| b == u8::MAX)
            && self.io.program(slot, 0, &bytes[..COMMIT_AT])
            && self.io.program(slot, COMMIT_OFFSET, &COMMIT)
            && self.io.read(slot, 0, &mut checked)
            && bytes == checked;
        if !ok {
            self.status = NetStatus::Io;
            return false;
        }
        self.slot = slot;
        self.generation = generation;
        self.credentials = credentials.copied();
        true
    }
}
