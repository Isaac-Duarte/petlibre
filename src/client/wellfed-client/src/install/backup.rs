//! The stock backup that promotion waits for ([rollback]).
//!
//! Promotion writes the replacement into bank 1, which until then holds
//! the feeder's only copy of stock. So an install never promotes until it
//! holds a [`VerifiedBackup`], and the only way to make one is
//! [`VerifiedBackup::verify`]: a whole stock OTA image whose SHA-256 is the
//! pinned 1.1.15 image's. A damaged or partial copy cannot pass, and the
//! check does not care where the bytes came from.
//!
//! Where they come from is the rollback card's: the build on bank 0
//! streams bank 1 over the device API, and the installer rebuilds the OTA
//! container from it (restoring the marker byte stock's activation zeroed).
//! Until that stream exists, a caller may hand in a copy the owner already
//! holds, which passes the same check.
//!
//! [rollback]: ../../../../../docs/firmware/consumer-installation.md#rollback

use sha2::{Digest, Sha256};

/// The stock release an install starts from and a backup restores.
pub const STOCK_VERSION: &str = "1.1.15";

/// SHA-256 of the whole stock 1.1.15 OTA image (container header
/// included), as pinned in the device layout and the image profile.
pub const STOCK_OTA_SHA256: [u8; 32] = [
    0x05, 0x14, 0x74, 0x76, 0xfc, 0x70, 0xc9, 0x1c, 0xc7, 0x7a, 0xd7, 0xda, 0x6d, 0x67, 0x72, 0x37,
    0x6b, 0x20, 0x81, 0xd4, 0x60, 0xac, 0x7b, 0x19, 0x12, 0xe2, 0xd7, 0x38, 0x68, 0xa0, 0x94, 0x98,
];

/// Why a backup does not count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BackupError {
    /// The bytes are not the pinned stock image: damaged, partial, or
    /// another release.
    #[error("the backup is not stock {STOCK_VERSION}'s pinned image")]
    NotPinned { sha256: [u8; 32] },
}

/// Proof that a copy of stock matching the pinned image exists. It holds
/// no bytes: the caller keeps the file (and offers it to the owner to
/// export); the install only needs to know it was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedBackup {
    sha256: [u8; 32],
}

impl VerifiedBackup {
    /// Check a whole stock OTA image against the pinned one.
    pub fn verify(image: &[u8]) -> Result<Self, BackupError> {
        let sha256: [u8; 32] = Sha256::digest(image).into();
        if sha256 != STOCK_OTA_SHA256 {
            return Err(BackupError::NotPinned { sha256 });
        }
        Ok(Self { sha256 })
    }

    /// The backup's digest, which is the pinned one.
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }

    /// The stock release it restores.
    pub fn version(&self) -> &'static str {
        STOCK_VERSION
    }

    /// A backup for this crate's tests, which cannot hold the vendor
    /// image. No other build can forge one.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            sha256: STOCK_OTA_SHA256,
        }
    }
}
