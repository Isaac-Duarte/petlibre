//! The crate's error type: each module's refusals wrapped whole, so a caller
//! tells them apart by variant and the message an operator reads is the
//! module's own.
//!
//! `Io` and `Json` stay their own variants: the flash client records an I/O
//! or JSON failure under its own kind and every other variant as a refusal.

use std::io;

use crate::{bank, ota, review, root};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Ota(#[from] ota::Error),
    #[error(transparent)]
    Bank(#[from] bank::Error),
    #[error(transparent)]
    Root(#[from] root::Error),
    #[error(transparent)]
    Review(#[from] review::Error),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
