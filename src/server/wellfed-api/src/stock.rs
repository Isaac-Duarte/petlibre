//! The feeder's own stock firmware, as the server keeps it: the verified
//! backup an installer read from bank 1 before promotion overwrote it, and
//! "Restore stock", which installs it back into bank 1.
//!
//! The server never signs. A restore carries the manifest and the
//! downgrade statement the owner signed on their own computer with their
//! update key (`wellfed server stock restore`), and the server checks that
//! the manifest offers exactly the copy it keeps before anything is sent.

use jiff::Timestamp;
use wellfed_model::Hex;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{FeederId, Install};

/// A signed version 2 manifest's length, signature included.
pub const MANIFEST_LEN: usize = 160;
/// A signed downgrade statement's length, signature included.
pub const STATEMENT_LEN: usize = 148;

/// `GET /feeders/{id}/stock`: the backup the server keeps for the feeder,
/// what the feeder says of its banks now, and the last restore.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StockState {
    pub feeder: FeederId,
    /// The copy the server keeps; `None` until one is read from the feeder
    /// or uploaded.
    pub backup: Option<StockBackup>,
    /// Read from the feeder for this request, for an owner only; `None`
    /// for anyone else, while it is offline, or where the server's key is
    /// not an owner's (the feeder answers its update state to owners only).
    pub feeder_state: Option<StockFeeder>,
    /// The feeder's running restore, or else its last one.
    pub restore: Option<Install>,
}

/// The stock image the server keeps for one feeder, in its data directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StockBackup {
    /// The stock release it is (`1.1.15`).
    pub version: String,
    /// Whether the kept file still matches the pinned image: checked again
    /// on every read, and only a verified copy is exported or restored.
    pub state: StockBackupState,
    /// SHA-256 of the file: the vendor's OTA container, byte for byte.
    #[schema(value_type = String)]
    pub sha256: Hex<32>,
    pub bytes: u64,
    pub source: StockSource,
    /// When the server kept it.
    pub kept: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StockBackupState {
    /// The file matches stock's pinned image.
    Verified,
    /// The file no longer matches it (changed or damaged on disk): it is
    /// neither exported nor restored.
    Damaged,
}

/// Where the kept copy came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StockSource {
    /// Read from bank 1 by the server, while the feeder ran the
    /// replacement from bank 0.
    Feeder,
    /// A copy an owner uploaded: the one the installer kept, or an export.
    Upload,
}

/// What the feeder says of its banks, as a restore or a read needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StockFeeder {
    /// The bank the feeder runs from.
    pub bank: u8,
    /// The highest confirmed version: a restore's downgrade statement is
    /// signed for this floor.
    pub floor: u32,
    /// Whether the feeder serves bank 1 now: only the replacement's
    /// bank-0 recovery role does, before promotion.
    pub readable: bool,
}

/// `POST /feeders/{id}/stock/restore`: the kept backup, signed as the
/// stock image by the owner's update key, and the downgrade statement for
/// this feeder at its floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StockRestore {
    /// The version 2 manifest of the stock kind that names the kept
    /// backup's bank contents.
    #[schema(value_type = String)]
    pub manifest: Hex<MANIFEST_LEN>,
    /// The downgrade statement bound to this feeder, the manifest and the
    /// floor `GET /feeders/{id}/stock` reports.
    #[schema(value_type = String)]
    pub downgrade: Hex<STATEMENT_LEN>,
}
