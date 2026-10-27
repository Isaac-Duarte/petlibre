//! What the feeder reports about itself, and its settings.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Generation;

/// What the feeder reports about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = FeederStatus)]
pub struct Status {
    pub build: String,
    /// The flash bank it booted from.
    pub bank: u8,
    pub uptime_ms: u32,
    pub boot_count: u32,
    pub reset_flags: u32,
    pub faults: u32,
    /// Wi-Fi signal, dBm.
    pub rssi: i32,
    pub wifi: WifiSource,
    pub link_losses: u32,
    /// Losses the feeder's link check noticed (its DHCP lease ran out or
    /// its gateway stopped answering) rather than the association. The name
    /// is from the internet DNS check that check replaced.
    pub dns_losses: u32,
    /// The feeder's clock; `None` while it is invalid.
    pub clock: Option<Timestamp>,
    pub power: Option<Power>,
    /// The panel's child lock; `None` before it is known.
    pub locked: Option<bool>,
    pub settings_generation: Generation,
    pub wifi_generation: Generation,
    /// The newest event's sequence number on the feeder.
    pub last_event: Option<u64>,
    /// Open device API sessions, the server's included.
    pub sessions: u8,
    /// Whether a feed can start now; absent from a feeder that does not
    /// report it.
    pub readiness: Option<Readiness>,
}

/// Whether the feeder can start a feed now: the one decision a feed
/// request obeys too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Blocked(Blocked),
}

/// Why a feed cannot start now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    /// The feeding owner has not opened yet.
    Starting,
    /// The feed journal could not be opened: no feed this boot.
    Journal,
    /// The feeding owner stopped on a fault: no feed until a restart.
    Fault,
    /// A feed a reset interrupted is being resolved as uncertain.
    Resolving,
    /// A feed is queued or runs.
    Feeding,
    /// A Wi-Fi join runs or waits.
    Joining,
    /// An install holds the flash.
    Updating,
    /// BLE setup mode runs, which ends in a restart.
    Setup,
    /// The firmware has no feeding owner.
    Unserved,
    /// On its batteries, below the floor a feed needs (or unread), or
    /// after a brownout until the adapter returns.
    Battery,
}

impl Blocked {
    pub const ALL: [Self; 10] = [
        Self::Starting,
        Self::Journal,
        Self::Fault,
        Self::Resolving,
        Self::Feeding,
        Self::Joining,
        Self::Updating,
        Self::Setup,
        Self::Unserved,
        Self::Battery,
    ];

    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Journal => "journal",
            Self::Fault => "fault",
            Self::Resolving => "resolving",
            Self::Feeding => "feeding",
            Self::Joining => "joining",
            Self::Updating => "updating",
            Self::Setup => "setup",
            Self::Unserved => "unserved",
            Self::Battery => "battery",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.code() == code)
    }
}

/// Which Wi-Fi credentials the feeder joined with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WifiSource {
    Build,
    Stored,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Power {
    pub adapter: bool,
    pub battery_mv: Option<u32>,
}

/// The battery pack, as the feeder judged its reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Battery {
    /// No pack fitted.
    Absent,
    Reading {
        mv: u16,
    },
    /// A value no pack of the board can read, as read.
    Implausible {
        mv: u16,
    },
    /// Nothing sampled.
    Unread,
    /// The last sample is this old, past the feeder's limit.
    Stale {
        age_ms: u32,
    },
}

/// The feeder's settings. Its `generation` goes back in `If-Match` with a
/// change, and is the `ETag` of a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Settings {
    pub generation: Generation,
    pub lights: bool,
    pub sound: bool,
    /// 0-100.
    pub volume: u8,
    /// Whether the settings store can take a write now.
    pub writable: bool,
}

/// The settings to change; absent fields stay as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SettingsPatch {
    #[serde(default)]
    pub lights: Option<bool>,
    #[serde(default)]
    pub sound: Option<bool>,
    /// 0-100.
    #[serde(default)]
    pub volume: Option<u8>,
}
