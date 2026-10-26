//! What the device API's requests hand the firmware's owners, and how
//! each request ended: settings, the panel lock, the clock, a tune, the
//! Wi-Fi credentials and the client store. The owner tasks fill these from their own state; the
//! device API only formats them.
//!
//! Sans-IO and allocation-free, like the rest of the crate. The settings'
//! and a tune's types are the SDK's: its settings mailbox and speaker carry
//! them for every firmware.

use wellfed_core::clock::wall::{Jump, Sync, SyncError};
pub use wellfed_core::clock::wall::{Origin, State as ClockState};
pub use wellfed_core::clock::zone::Zone;
use wellfed_core::named_enum;

use crate::clients::{Label, Role};

pub use wellfed_core::settings::{MAX_VOLUME, SettingsFields, SettingsResult, SettingsView};
pub use wellfed_core::tone::PlayResult;

/// The fields of a `lock` request; none present reads the lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockFields {
    pub locked: Option<bool>,
}

impl LockFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.locked.is_some()
    }
}

/// The fields of a `time` request, as sent (validated by the clock).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeFields {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

/// How a `time` request ended, for its reply: the jump against the running
/// clock in milliseconds (`None` for a first sync), or the refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeResult {
    Synced { jump_ms: Option<i64> },
    Refused(&'static str),
}

impl TimeFields {
    /// The sync the wall clock checks and adopts.
    pub const fn sync(self) -> Sync {
        Sync {
            unix_ms: self.unix_ms,
            offset_s: self.offset_s,
            next_change_s: self.next_change_s,
            next_offset_s: self.next_offset_s,
        }
    }
}

impl TimeResult {
    /// How the wall clock took a sync, as the reply reports it.
    pub const fn from_sync(result: Result<Jump, SyncError>) -> Self {
        match result {
            Ok(Jump::First) => Self::Synced { jump_ms: None },
            Ok(Jump::Step(delta) | Jump::Jumped(delta)) => Self::Synced {
                jump_ms: Some(delta),
            },
            Err(SyncError::Time) => Self::Refused("time"),
            Err(SyncError::Offset) => Self::Refused("offset"),
            Err(SyncError::NextChange) => Self::Refused("next_change"),
            Err(SyncError::Bounded) => Self::Refused("bounded"),
        }
    }
}

/// Where the feeder asks for the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtpServer {
    Off,
    /// The router that gave the feeder its address: on a home network the
    /// one host the feeder already trusts to route it.
    Gateway {
        port: u16,
    },
    Address {
        ip: [u8; 4],
        port: u16,
    },
}

impl Default for NtpServer {
    fn default() -> Self {
        Self::Gateway {
            port: wellfed_core::clock::ntp::PORT,
        }
    }
}

/// The time settings: the owner's zone and the NTP server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeSettings {
    /// `None`: local time comes from a client sync's offsets, if any.
    pub zone: Option<Zone>,
    pub ntp: NtpServer,
}

impl TimeSettings {
    /// A checksum of the settings, so a pending write can tell whether
    /// the store now holds what it asked for without keeping a copy.
    pub fn digest(&self) -> u32 {
        let mut bytes = [0; wellfed_core::clock::zone::TEXT_MAX + 8];
        let text = self.zone.as_ref().map_or(&[][..], Zone::text);
        bytes[..text.len()].copy_from_slice(text);
        let (kind, ip, port) = match self.ntp {
            NtpServer::Off => (0, [0; 4], 0),
            NtpServer::Gateway { port } => (1, [0; 4], port),
            NtpServer::Address { ip, port } => (2, ip, port),
        };
        let tail = wellfed_core::clock::zone::TEXT_MAX;
        bytes[tail] = u8::try_from(text.len()).unwrap_or(u8::MAX);
        bytes[tail + 1] = kind;
        bytes[tail + 2..tail + 6].copy_from_slice(&ip);
        bytes[tail + 6..tail + 8].copy_from_slice(&port.to_le_bytes());
        wellfed_core::store::slot::crc32(&bytes)
    }
}

/// The time settings as the writer last published them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeSettingsView {
    pub settings: TimeSettings,
    pub generation: u32,
    pub writable: bool,
}

/// How a time settings request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeSettingsResult {
    /// The current settings; `pending` once a change is queued for the
    /// writer.
    Current {
        view: TimeSettingsView,
        pending: bool,
    },
    /// A change is already queued, or the store is still loading.
    Busy,
    ReadOnly,
    /// This firmware keeps no time settings.
    Unserved,
}

/// What the last NTP exchange came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtpResult {
    Used,
    /// The answer disagreed with a valid clock beyond both error bounds.
    Bounded,
    Lost,
    /// Unusable: unsynchronized, kiss-o'-death, too loose or malformed.
    Refused,
}

/// The clock and the NTP exchange, for the time view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeState {
    pub clock: ClockState,
    /// The uptime now, to age the last fix.
    pub now_ms: u32,
    pub ntp_last: Option<NtpResult>,
}

/// How a `lock` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockResult {
    /// The lock as the panel's owner last published it; `pending` once a
    /// change is queued for that owner.
    Current { locked: bool, pending: bool },
    /// A change is already queued.
    Busy,
    /// This firmware runs no panel owner, so there is no lock to set.
    Unserved,
}

/// Which credentials the Wi-Fi join uses this boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WifiSource {
    #[default]
    Build,
    Stored,
    /// Stored credentials failed to join; the build's took over this boot.
    Fallback,
}

impl WifiSource {
    /// The source as the C join shim and the port's atomics carry it.
    pub const fn code(self) -> u8 {
        match self {
            Self::Build => 0,
            Self::Stored => 1,
            Self::Fallback => 2,
        }
    }

    /// The inverse of `code`; anything else reads as the build's.
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Stored,
            2 => Self::Fallback,
            _ => Self::Build,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Stored => "stored",
            Self::Fallback => "fallback",
        }
    }
}

/// The Wi-Fi credentials as a `wifi` reply reports them: never the passphrase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiView {
    /// CRC-32 of the stored SSID; `None` when the build's credentials apply.
    pub stored_ssid_crc: Option<u32>,
    pub generation: u32,
    pub writable: bool,
    pub source: WifiSource,
}

/// How a `wifi` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiResult {
    /// The current state; `pending` once a change is queued for the writer.
    Current {
        view: WifiView,
        pending: bool,
    },
    Busy,
    ReadOnly,
    /// The SSID or passphrase failed the store's checks.
    Invalid,
}

/// A change to the client store, queued for the settings writer, which
/// alone writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientChange {
    Enroll {
        key: [u8; 32],
        label: Label,
        role: Role,
    },
    Revoke {
        key: [u8; 32],
    },
    /// Forget every client and take a new identity.
    Reset,
}

named_enum! {
    /// Whether a one-slot mailbox took a change.
    pub enum Requested {
        Queued = "queued",
        /// A change is already waiting for the writer.
        Busy = "busy",
    }
}

named_enum! {
    /// What became of a queued client store change.
    pub enum KeyResult {
        Done = "done",
        Unarmed = "unarmed",
        /// A reset needs a new identity, and the generator never started.
        NoGenerator = "no-generator",
        NotWritable = "not-writable",
        Full = "full",
        Duplicate = "duplicate",
        NotFound = "not-found",
        Io = "io",
    }
}

named_enum! {
    /// The panel's child lock, which a `lock` request reads or sets.
    pub enum Lock {
        Locked = "locked",
        Unlocked = "unlocked",
    }
}

impl Lock {
    pub const fn from_locked(locked: bool) -> Self {
        if locked { Self::Locked } else { Self::Unlocked }
    }

    pub const fn locked(self) -> bool {
        matches!(self, Self::Locked)
    }
}
