//! Running the server itself: releases and installs (a restore of stock
//! among them), backups, export and import, the background jobs' bodies,
//! and readiness.

mod backups;
pub(crate) mod export;
mod health;
mod import;
pub(crate) mod installs;
mod jobs;
pub(crate) mod releases;
mod stock;

use std::sync::Arc;

pub use backups::{BackupError, Backups};
pub use export::{CHUNK as EXPORT_CHUNK, ExportError};
pub use installs::InstallError;
pub use stock::StockError;

/// The operations that run one at a time per server.
#[derive(Clone, Default)]
pub(crate) struct State {
    /// Backups are taken one at a time.
    pub(crate) backing_up: Arc<tokio::sync::Mutex<()>>,
    /// Held by the export being written: one runs at a time, so exports
    /// hold at most one reader and one snapshot.
    pub(crate) exporting: Arc<tokio::sync::Mutex<()>>,
    /// The one install the server runs at a time.
    pub(crate) installing: installs::Slot,
}
