//! What the device API's requests hand the firmware's owners, and how
//! each request ended: settings, the panel lock, the clock, a tune, the
//! Wi-Fi credentials and the client store. The owner tasks fill these from their own state; the
//! device API only formats them.
//!
//! Sans-IO and allocation-free, like the rest of the crate. The settings'
//! and a tune's types are the SDK's: its settings mailbox and speaker carry
//! them for every firmware.

use wellfed_core::clock::wall::{Jump, Sync, SyncError};
use wellfed_core::named_enum;

use crate::store::clients::{ClientError, Label, Role};

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
        }
    }
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

impl From<ClientError> for KeyResult {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::NotWritable | ClientError::IdentitySet => Self::NotWritable,
            ClientError::Full => Self::Full,
            ClientError::Duplicate => Self::Duplicate,
            ClientError::NotFound => Self::NotFound,
            ClientError::Io => Self::Io,
        }
    }
}
