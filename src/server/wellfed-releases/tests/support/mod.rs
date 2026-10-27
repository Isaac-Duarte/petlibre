//! What both test files build: the keys, release files signed the way
//! `wellfed-build sign` signs them, and the cache in a temporary
//! directory.

use ed25519_dalek::{Signer as _, SigningKey};
use jiff::Timestamp;
use wellfed_api::ReleaseQuery;
use wellfed_core::ota::offer::{self, ImageKind, Signed, key_id};
use wellfed_core::ota::{MARKER, Manifest, SECTOR};
use wellfed_releases::{Config, Keys, Releases, Source, StockConfig, StockPins};
use sha2_0_10::{Digest, Sha256};
use tempfile::TempDir;

pub fn project() -> SigningKey {
    SigningKey::from_bytes(&[0x05; 32])
}

pub fn owner() -> SigningKey {
    SigningKey::from_bytes(&[0x06; 32])
}

pub fn keys() -> Keys {
    Keys {
        project: vec![project().verifying_key().to_bytes()],
        owner: vec![owner().verifying_key().to_bytes()],
    }
}

pub fn now() -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap()
}

/// A release at `version`, signed by `key`.
pub fn release(key: &SigningKey, version: u32) -> Vec<u8> {
    let mut image: Vec<u8> = (0..2 * SECTOR)
        .map(|i| (i % 251) as u8 ^ version as u8)
        .collect();
    image[..8].copy_from_slice(&MARKER);
    let signed = Signed {
        manifest: Manifest {
            version,
            image_len: u32::try_from(image.len()).unwrap(),
            target_bank: 1,
            image_sha256: Sha256::digest(&image).into(),
            build_id: [version as u8; 32],
        },
        kind: ImageKind::Replacement,
        key_id: key_id(&key.verifying_key().to_bytes()),
    };
    let body = offer::manifest_body(&signed);
    let mut file = body.to_vec();
    file.extend_from_slice(&key.sign(&body).to_bytes());
    file.extend_from_slice(&image);
    file
}

pub async fn open(dir: &TempDir, fetch: Option<Source>) -> Releases {
    open_with(dir, keys(), fetch).await
}

pub async fn open_with(dir: &TempDir, keys: Keys, fetch: Option<Source>) -> Releases {
    Releases::open(Config {
        dir: dir.path().join("releases"),
        keys,
        fetch,
        stock: StockConfig {
            dir: dir.path().join("stock"),
            pins: stock_pins(),
        },
    })
    .await
    .unwrap()
}

/// A synthetic stock bank: stock's size, starting with the marker. The
/// real image is the vendor's and stays out of the repository.
pub fn stock_bank() -> Vec<u8> {
    let mut bank: Vec<u8> = (0..0x15_0000u32)
        .map(|i| i.to_le_bytes()[0] ^ i.to_le_bytes()[1] ^ 0x5a)
        .collect();
    bank[..8].copy_from_slice(&MARKER);
    bank
}

/// The pins a test cache checks stock against: [`stock_bank`]'s.
pub fn stock_pins() -> StockPins {
    StockPins::of_payload(&stock_bank())
}

pub async fn versions(releases: &Releases) -> Vec<u32> {
    let page = releases.list(&ReleaseQuery::default()).await.unwrap();
    page.items.iter().map(|release| release.version).collect()
}
