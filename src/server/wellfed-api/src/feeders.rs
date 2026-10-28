//! Adopted feeders, their status, settings and lock.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use wellfed_model::{ClientRole, Hex, Status};

use crate::FeederId;

/// Adopt a feeder by redeeming an invite its owner created for the server
/// (`wellfed invite --role server`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AdoptFeeder {
    /// The invite's text, `wellfed-invite:1:…`. It holds a secret, so it
    /// is never stored or logged.
    pub invite: String,
    /// What the household calls the feeder (1-64 characters).
    pub name: String,
    /// The feeder's IP address (the device API's port) or `IP:port`; a DNS
    /// name is refused. Found over mDNS when absent, and required while
    /// discovery is off.
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
    /// When it was removed from the server; `None` while it is mirrored.
    pub archived: Option<Timestamp>,
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
    pub status: Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LockRequest {
    pub locked: bool,
}
