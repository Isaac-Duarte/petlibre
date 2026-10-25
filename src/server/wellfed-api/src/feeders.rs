//! Adopted feeders, their status, settings and lock.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::events::ClientRole;
use crate::{FeederId, Generation, Hex};

/// Adopt a feeder by redeeming an invite its owner created for the server
/// (`wellfed invite --role server`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AdoptFeeder {
    /// The invite's text, `wellfed-invite:1:…`. It holds a secret, so it
    /// is never stored or logged.
    pub invite: String,
    /// What the household calls the feeder (1-64 characters).
    pub name: String,
    /// The feeder's `host` or `host:port`; found over mDNS when absent.
    #[serde(default)]
    pub address: Option<String>,
}

impl fmt::Debug for AdoptFeeder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdoptFeeder")
            .field("name", &self.name)
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// The server's session with a feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    Offline,
    Connecting,
    Online,
    /// The feeder no longer knows the server's key (revoked, or reset).
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Feeder {
    pub id: FeederId,
    pub name: String,
    /// The feeder's identity key.
    #[schema(value_type = String)]
    pub identity: Hex<32>,
    /// The role the feeder enrolled the server's key with.
    pub role: ClientRole,
    /// Where the server last reached it.
    pub address: Option<String>,
    pub connection: Connection,
    /// The newest event the server holds, `None` before the first.
    pub synced: Option<u64>,
    /// The feeder's history epoch `synced` counts in.
    pub epoch: Option<u32>,
}

/// One feeder and its latest status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederDetail {
    pub feeder: Feeder,
    /// `None` until the server has read it once.
    pub status: Option<StatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StatusSnapshot {
    /// When the server read it.
    pub at: Timestamp,
    /// Read for this request, rather than the last copy from before the
    /// feeder went offline.
    pub live: bool,
    pub status: FeederStatus,
}

/// What the feeder reports about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederStatus {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LockRequest {
    pub locked: bool,
}
