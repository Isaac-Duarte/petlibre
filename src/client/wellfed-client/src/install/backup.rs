//! The stock backup that promotion waits for ([rollback]).
//!
//! Promotion writes the replacement into bank 1, which until then holds
//! the feeder's only copy of stock. So an install never promotes until it
//! holds a [`VerifiedBackup`], and the only way to make one is
//! [`VerifiedBackup::verify`]: stock 1.1.15's bare application payload,
//! whose SHA-256 is the pinned payload's, or the vendor's exact OTA
//! container, whose SHA-256 is the pinned container's. A damaged or partial
//! copy cannot pass, and the check does not care where the bytes came from.
//!
//! The payload is pinned, not only the whole container, because the bank
//! holds the payload: a factory-flashed unit may never have been written
//! from that exact container, and a backup read from bank 1 is a payload.
//! A container is never taken apart, though: any header other than the
//! vendor's over the right payload is refused, since its fields are not
//! checked here and a restore would write it as given.
//!
//! Where the bytes come from is the rollback card's: the build on bank 0
//! streams bank 1 over the device API, and the installer restores the
//! marker byte stock's activation zeroed. Until that stream exists, a caller
//! may hand in a copy the owner already holds, which passes the same check.
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

/// SHA-256 of that image's payload (everything after its 32-byte
/// header): the bank's contents, which the stock review records as its
/// `payload_sha256`.
pub const STOCK_PAYLOAD_SHA256: [u8; 32] = [
    0x80, 0x2e, 0xf6, 0x20, 0x4c, 0x2d, 0xce, 0x8a, 0xf8, 0xb7, 0x4d, 0x78, 0xc5, 0xb5, 0x8b, 0x1d,
    0xc5, 0xf9, 0x12, 0xc2, 0x04, 0xbd, 0x0a, 0xc6, 0x8e, 0x0a, 0x03, 0xa7, 0x21, 0x20, 0x70, 0x5a,
];

/// Why a backup does not count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BackupError {
    /// The bytes are neither the pinned payload nor a container of it:
    /// damaged, partial, or another release.
    #[error("the backup is not stock {STOCK_VERSION}'s pinned image")]
    NotPinned { sha256: [u8; 32] },
}

/// Proof that a copy of stock matching the pinned payload exists. It holds
/// no bytes: the caller keeps the file (and offers it to the owner to
/// export); the install only needs to know it was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedBackup {
    _checked: (),
}

impl VerifiedBackup {
    /// Check a bare stock payload, or the exact pinned OTA container,
    /// against the pins.
    pub fn verify(image: &[u8]) -> Result<Self, BackupError> {
        verify_against(image, &STOCK_OTA_SHA256, &STOCK_PAYLOAD_SHA256)
    }

    /// The stock release it restores.
    pub fn version(&self) -> &'static str {
        STOCK_VERSION
    }

    /// A backup for this crate's tests, which cannot hold the vendor
    /// image. No other build can forge one.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self { _checked: () }
    }
}

/// `image` is exactly the pinned container, or exactly the pinned payload.
/// A container is never taken apart: a header other than the vendor's
/// (lengths, byte-sum, magic) over the right payload is refused, since
/// nothing here checks its fields and a restore would write it as given.
fn verify_against(
    image: &[u8],
    container: &[u8; 32],
    payload: &[u8; 32],
) -> Result<VerifiedBackup, BackupError> {
    let sha256: [u8; 32] = Sha256::digest(image).into();
    if sha256 == *container || sha256 == *payload {
        Ok(VerifiedBackup { _checked: () })
    } else {
        Err(BackupError::NotPinned { sha256 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic pins over synthetic bytes: the real payload is the vendor's
    /// and stays out of the repository.
    #[test]
    fn only_the_exact_container_or_the_bare_payload_passes() {
        let payload = b"stock payload, synthetic".to_vec();
        let mut container = vec![0x11; 32];
        container.extend_from_slice(&payload);
        let pins = (
            <[u8; 32]>::from(Sha256::digest(&container)),
            <[u8; 32]>::from(Sha256::digest(&payload)),
        );
        let check = |image: &[u8]| verify_against(image, &pins.0, &pins.1).is_ok();

        assert!(check(&container));
        assert!(check(&payload));

        // The right payload under a damaged header is refused.
        let mut corrupt = container.clone();
        corrupt[4] ^= 0x01;
        assert!(!check(&corrupt));
        // So are a truncated payload and a payload with bytes appended.
        assert!(!check(&payload[..payload.len() - 1]));
        assert!(!check(&[&payload[..], &[0]].concat()));
    }
}
