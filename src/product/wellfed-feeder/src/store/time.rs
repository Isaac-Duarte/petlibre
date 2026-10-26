//! The time settings on NOR: the owner's zone and the NTP server.
//!
//! A 128-byte record in the dual-slot store
//! (`wellfed_core::store::slot`), as the Wi-Fi credentials are: magic
//! `PLFTIM01`, version, record size, generation, the NTP server (kind at 20:
//! 0 off, 1 the gateway, 2 an address; the IPv4 address at 21..25 and the
//! port at 25..27, little-endian), the zone rule's length at 27 and its text
//! at 28..76 (zero when no zone is set), zero padding, CRC-32/ISO-HDLC of
//! bytes 0..120 at 120 and the `CMIT` commit marker at 124.
//!
//! These two sectors were free before this store, so a build without it
//! never reads or writes them, and the settings record stays as it was: a
//! bank that predates the store runs on its own settings and ignores these.
//! The newest valid record wins; a torn or invalid newer slot falls back to
//! the older one. With no record the defaults apply (no zone, NTP from the
//! gateway on port 123). Only a newer schema version makes the store
//! read-only, with the defaults.

use core::ffi::c_uint;

use wellfed_core::clock::zone::{TEXT_MAX, Zone};
use wellfed_core::store::slot::{
    CRC_AT, DualSlot, Extent, Layout, Pick, Prepare, RECORD_BYTES, SlotIo, get_u16, get_u64,
    newest, put_u16, put_u64, seal, target,
};
use wellfed_device_api::mailbox::{NtpServer, TimeSettings};

pub const VERSION: u16 = 1;
const LAYOUT: Layout = Layout {
    magic: *b"PLFTIM01",
    version: VERSION,
    extent: Extent::Record,
};
const GENERATION_AT: usize = 12;
const NTP_AT: usize = 20;
const IP_AT: usize = 21;
const PORT_AT: usize = 25;
const ZONE_LEN_AT: usize = 27;
const ZONE_AT: usize = 28;
const ZONE_END: usize = ZONE_AT + TEXT_MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeStatus {
    Unopened,
    /// No record yet: the defaults, writable.
    Empty,
    Loaded,
    /// A newer schema version: the defaults, read-only.
    ReadOnly,
    Io,
}

impl TimeStatus {
    /// The status's record word.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unopened => "unopened",
            Self::Empty => "empty",
            Self::Loaded => "loaded",
            Self::ReadOnly => "read-only",
            Self::Io => "io",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimeStore<I> {
    units: DualSlot<I>,
    pub status: TimeStatus,
    pub generation: u64,
    pub slot: c_uint,
    pub settings: TimeSettings,
}

impl<I> Default for TimeStore<I> {
    fn default() -> Self {
        Self {
            units: DualSlot::unbound(&LAYOUT),
            status: TimeStatus::Unopened,
            generation: 0,
            slot: 0,
            settings: TimeSettings::default(),
        }
    }
}

fn encode(generation: u64, settings: &TimeSettings) -> [u8; RECORD_BYTES] {
    let mut p = LAYOUT.record();
    put_u64::<GENERATION_AT, _>(&mut p, generation);

    let (kind, ip, port) = match settings.ntp {
        NtpServer::Off => (0, [0; 4], 0),
        NtpServer::Gateway { port } => (1, [0; 4], port),
        NtpServer::Address { ip, port } => (2, ip, port),
    };
    p[NTP_AT] = kind;
    p[IP_AT..IP_AT + 4].copy_from_slice(&ip);
    put_u16::<PORT_AT, _>(&mut p, port);

    if let Some(zone) = &settings.zone {
        let text = zone.text();
        // `Zone::parse` bounds the text at `TEXT_MAX`, under a byte.
        p[ZONE_LEN_AT] = u8::try_from(text.len()).expect("a zone rule is at most 48 bytes");
        p[ZONE_AT..ZONE_AT + text.len()].copy_from_slice(text);
    }
    seal(&mut p);
    p
}

/// The generation and the settings of a record whose header and checksum
/// hold.
fn decode(p: &[u8; RECORD_BYTES]) -> Option<(u64, TimeSettings)> {
    let generation = get_u64::<GENERATION_AT, _>(p);
    let ip: [u8; 4] = p[IP_AT..IP_AT + 4].try_into().ok()?;
    let port = get_u16::<PORT_AT, _>(p);
    let ntp = match p[NTP_AT] {
        0 if ip == [0; 4] && port == 0 => NtpServer::Off,
        1 if ip == [0; 4] && port != 0 => NtpServer::Gateway { port },
        2 if port != 0 => NtpServer::Address { ip, port },
        _ => return None,
    };

    // Unused bytes must be zero, so one encoding exists per value.
    let len = usize::from(p[ZONE_LEN_AT]);
    if generation == 0 || len > TEXT_MAX || p[ZONE_AT + len..CRC_AT].iter().any(|&b| b != 0) {
        return None;
    }
    let zone = match len {
        0 => None,
        _ => Some(Zone::parse(&p[ZONE_AT..ZONE_AT + len]).ok()?),
    };
    Some((generation, TimeSettings { zone, ntp }))
}

const _: () = assert!(ZONE_END <= CRC_AT);

impl<I: SlotIo> TimeStore<I> {
    /// Bind to two exclusive erase units and load the newest valid record.
    pub fn open(&mut self, io: I) -> TimeStatus {
        let unit = io.unit_bytes();
        *self = Self {
            units: DualSlot::new(io, &LAYOUT),
            ..Self::default()
        };
        if (unit as usize) < RECORD_BYTES {
            self.status = TimeStatus::Io;
            return self.status;
        }
        let (Some(a), Some(b)) = (self.units.scan(0, decode), self.units.scan(1, decode)) else {
            self.status = TimeStatus::Io;
            return self.status;
        };

        self.status = match newest(a, b, |&(generation, _)| generation) {
            Pick::Future => TimeStatus::ReadOnly,
            Pick::Newest(slot, (generation, settings)) => {
                self.slot = slot;
                self.generation = generation;
                self.settings = settings;
                TimeStatus::Loaded
            }
            // The two sectors are this store's alone, so anything else there
            // is a save of ours cut short: the defaults, still writable.
            Pick::Nothing => TimeStatus::Empty,
        };
        self.status
    }

    pub fn writable(&self) -> bool {
        matches!(self.status, TimeStatus::Empty | TimeStatus::Loaded)
    }

    /// Store `settings` to the other slot by the torn-write rule. A failure
    /// keeps the previous settings in memory.
    pub fn save(&mut self, settings: &TimeSettings) -> bool {
        if !self.writable() || !self.units.writable() {
            return false;
        }
        let generation = self.generation + 1;
        let slot = target(self.generation, self.slot);
        if !self
            .units
            .write(slot, &encode(generation, settings), Prepare::Erase)
        {
            self.status = TimeStatus::Io;
            return false;
        }
        self.slot = slot;
        self.generation = generation;
        self.settings = *settings;
        true
    }
}
