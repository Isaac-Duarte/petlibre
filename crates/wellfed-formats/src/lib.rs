//! The files one host tool writes and another reads, typed once.
//!
//! Build manifests, the stock layout, the SDK and toolchain locks, serial
//! captures, the credential files and the flash client's reviewed plans each
//! cross crate boundaries (the builder writes what the audit, flash review
//! and bench read; the bench summarizes the flash plans), so their shape
//! lives here rather than as `Value` lookups in every reader.
//!
//! Whatever is hashed goes through [`canonical`], which fixes the bytes
//! independently of field order; other files are written in declaration
//! order and read as objects.

pub mod canonical;
pub mod capture;
pub mod credentials;
pub mod flash_plan;
pub mod hex;
pub mod layout;
pub mod lock;
pub mod manifest;
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
