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
    /// Losses a failed DNS check noticed rather than the link.
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
