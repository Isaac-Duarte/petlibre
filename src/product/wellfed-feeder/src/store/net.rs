//! Wi-Fi credentials stored on NOR, set without a rebuild.
//!
//! A 128-byte record in the dual-slot store
//! (`wellfed_core::store::slot`), as the settings are: magic `PLFNET01`,
//! version, record size, generation, SSID length, passphrase length, SSID
//! (32 bytes), passphrase (64 bytes), zero padding, CRC-32/ISO-HDLC of bytes
//! 0..120 at 120 and the `CMIT` commit marker at 124.
//! A record with an empty SSID means "use the build's credentials", so a
//! clear keeps the generation rising. The newest valid record wins; a torn or
//! invalid newer slot falls back to the older one. The two sectors belong to
//! this store alone, so an unreadable record is a save of ours cut short and
//! may be overwritten. Only a newer schema version makes the store read-only;
//! then, as with no record, the build's credentials apply.

use core::ffi::c_uint;

use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u64, newest,
    put_u64, seal, target,
};

pub const VERSION: u16 = 1;
const LAYOUT: Layout = Layout {
    magic: *b"PLFNET01",
    version: VERSION,
    extent: Extent::Record,
};
const GENERATION_AT: usize = 12;
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

#[derive(Clone, Copy, Debug)]
pub struct NetStore<I> {
    units: DualSlot<I>,
    pub status: NetStatus,
    pub generation: u64,
    pub slot: c_uint,
    /// `None` means the build's credentials.
    pub credentials: Option<Credentials>,
}

impl<I> Default for NetStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: NetStatus::Unopened,
            generation: 0,
            slot: 0,
            credentials: None,
        }
    }
}

fn encode(generation: u64, credentials: Option<&Credentials>) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u64::<GENERATION_AT, _>(&mut p, generation);
    if let Some(c) = credentials {
        p[20] = c.ssid_len;
        p[21] = c.psk_len;
        p[22..22 + MAX_SSID].copy_from_slice(&c.ssid);
        p[54..54 + 64].copy_from_slice(&c.psk);
    }
    seal(&mut p);
    p
}

/// The generation and the credentials, `None` for a clear.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u64, Option<Credentials>)> {
    let generation = get_u64::<GENERATION_AT, _>(p);
    let (ssid_len, psk_len) = (usize::from(p[20]), usize::from(p[21]));
    if generation == 0 || p[118..CRC_AT] != [0, 0] {
        return None;
    }
    if ssid_len == 0 {
        return (psk_len == 0 && p[22..118].iter().all(|&b| b == 0)).then_some((generation, None));
    }
    // Unused bytes must be zero, so one encoding exists per value.
    if ssid_len > MAX_SSID
        || psk_len > MAX_PSK
        || p[22 + ssid_len..54].iter().any(|&b| b != 0)
        || p[54 + psk_len..118].iter().any(|&b| b != 0)
    {
        return None;
    }
    Credentials::new(&p[22..22 + ssid_len], &p[54..54 + psk_len]).map(|c| (generation, Some(c)))
}

impl<I: SlotIo> NetStore<I> {
    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I) -> NetStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = NetStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.units.scan(0, decode), self.units.scan(1, decode)) else {
            self.status = NetStatus::Io;
            return self.status;
        };

        self.status = match newest(a, b, |&(generation, _)| generation) {
            Pick::Future => NetStatus::ReadOnly,
            Pick::Newest(slot, (generation, credentials)) => {
                self.slot = slot;
                self.generation = generation;
                self.credentials = credentials;
                NetStatus::Loaded
            }
            // The two sectors are this store's alone, so anything else there
            // is a save of ours cut short: no credentials, still writable.
            Pick::Nothing => NetStatus::Empty,
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, NetStatus::Empty | NetStatus::Loaded)
    }

    /// Store `credentials`, or `None` to return to the build's, to the
    /// other slot by the torn-write rule. A failure keeps the previous
    /// credentials in memory.
    pub fn save(&mut self, credentials: Option<&Credentials>) -> bool {
        if !self.writable() || !self.units.writable() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = target(self.generation, self.slot);
        if !self
            .units
            .write(slot, &encode(generation, credentials), Prepare::Erase)
        {
            self.status = NetStatus::Io;
            return false;
        }
        self.slot = slot;
        self.generation = generation;
        self.credentials = credentials.copied();
        true
    }
}
