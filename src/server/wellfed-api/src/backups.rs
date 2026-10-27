//! Copies of the server's database in `backups/` of its data directory:
//! one a day at 03:30 household time and one on each owner's request, the
//! newest `backups.keep` kept. A backup holds the feeder keys sealed, and
//! not the key that opens them (`keys/server.key`), which is backed up
//! apart; the directory's `README.txt` says so.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// One copy of the database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Backup {
    /// Its file name in `backups/`.
    pub name: String,
    /// When it was taken.
    pub taken: Timestamp,
    /// Its size in bytes.
    pub bytes: u64,
}

/// The backups kept, newest first. They are at most `backups.keep`, so
/// one page holds them all and `next` is always `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BackupPage {
    pub items: Vec<Backup>,
    /// The `after` for the next page; always `None`.
    pub next: Option<String>,
}
