//! The files one host tool writes and another reads, typed once.
//!
//! Build manifests, device configs, the stock layout, the SDK and toolchain locks, serial
//! captures, the credential and key files, the flash client's reviewed plans and the
//! QA firmware's UART records each cross crate boundaries (the builder writes
//! what the audit, flash review and bench read; the bench summarizes the flash
//! plans and follows a QA run; the bench and its suite write the progress
//! document and run records its page reads), so their shape lives here rather
//! than as `Value` lookups in every reader.
//!
//! Whatever is hashed goes through [`canonical`], which fixes the bytes
//! independently of field order; other files are written in declaration
//! order and read as objects.

#![forbid(unsafe_code)]

pub mod audit;
pub mod bench;
pub mod canonical;
pub mod capture;
pub mod credentials;
pub mod device;
pub mod flash_plan;
pub mod hex;
pub mod layout;
pub mod lock;
pub mod manifest;
pub mod owner;
pub mod private;
pub mod qa;
pub mod review;
pub mod session;
pub mod stock;
pub mod transients;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Canonical JSON carries integers only")]
    NotInteger,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

/// Read a JSON file into its type.
pub fn load<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

pub type Result<T> = std::result::Result<T, Error>;
