//! What the device API's requests hand the firmware's owners, and how
//! each request ended: settings, the panel lock, the clock, a tune and the
//! Wi-Fi credentials. The owner tasks fill these from their own state; the
//! device API only formats them.
//!
//! Sans-IO and allocation-free, like the rest of the crate.

/// The fields of a `settings` request; none present reads the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsFields {
    pub lights: Option<bool>,
    pub sound: Option<bool>,
    pub volume: Option<u8>,
}

impl SettingsFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.lights.is_some() || self.sound.is_some() || self.volume.is_some()
    }
}

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

/// Largest `volume` a `settings` request may set, and the store accepts.
pub const MAX_VOLUME: u8 = 100;

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

/// The settings as a `settings` reply reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    /// The stored record's generation; 0 for defaults never saved.
    pub generation: u32,
    /// Whether NOR holds a record this firmware can update.
    pub writable: bool,
}

/// How a `settings` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsResult {
    /// The current settings; `pending` once a change is queued for the writer.
    Current { view: SettingsView, pending: bool },
    /// Not loaded yet, or a change already pending.
    Busy,
    /// The store cannot be written (corrupt, newer schema, worn or unarmed).
    ReadOnly,
}

/// How a `play` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayResult {
    /// Queued for the speaker.
    Started,
    /// The `sound` setting is off.
    Muted,
    /// Settings not loaded yet, or a tune is playing.
    Busy,
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
