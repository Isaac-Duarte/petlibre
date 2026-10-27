//! Release files signed with a test key, as the server keeps and offers
//! them: the version 2 manifest, then the image it names. Also "Restore
//! stock" as the owner's CLI signs it, for a simulator's synthetic stock.

use std::cell::RefCell;

use ed25519_dalek::{Signer as _, SigningKey};
use wellfed_api::{Hex, StockRestore};
use wellfed_core::ota::offer::{self, ImageKind, Signed, key_id};
use wellfed_core::ota::{MARKER, Manifest};
use wellfed_feeder_sim::banks::Banks;
use wellfed_service::{Config, StockPins};
use sha2_0_10::{Digest, Sha256};

pub use wellfed_core::ota::SECTOR;
pub use wellfed_core::ota::offer::MANIFEST_LEN;

/// The project's key in every test that installs: the simulator trusts it
/// for updates, and the server for releases.
pub fn project() -> SigningKey {
    SigningKey::from_bytes(&[0x05; 32])
}

/// `config`, trusting [`project`] for releases.
pub fn trusting(mut config: Config) -> Config {
    config.releases.keys.project = vec![project().verifying_key().to_bytes()];
    config
}

/// `config`, keeping stock backups of the synthetic stock the simulator
/// on `banks` starts with in bank 1 (the vendor's image stays out of the
/// repository).
pub fn keeping_stock(mut config: Config, banks: &RefCell<Banks>) -> Config {
    config.releases.stock.pins = StockPins::of_payload(banks.borrow().bank1());
    config
}

/// "Restore stock" of `payload` (a kept backup's bank contents) at
/// `version`, signed by `key` as the stock image, with the downgrade
/// statement for the feeder `device` at `floor`: what the owner's CLI
/// signs.
pub fn stock_restore(
    key: &SigningKey,
    version: u32,
    payload: &[u8],
    device: [u8; 32],
    floor: u32,
) -> StockRestore {
    let sha256: [u8; 32] = Sha256::digest(payload).into();
    let signer = key_id(&key.verifying_key().to_bytes());
    let signed = Signed {
        manifest: Manifest {
            version,
            image_len: u32::try_from(payload.len()).expect("a bank fits a manifest"),
            target_bank: 1,
            image_sha256: sha256,
            build_id: sha256,
        },
        kind: ImageKind::Stock,
        key_id: signer,
    };
    let body = offer::manifest_body(&signed);
    let mut manifest = body.to_vec();
    manifest.extend_from_slice(&key.sign(&body).to_bytes());

    let digest = offer::manifest_digest(&manifest);
    let statement = offer::downgrade_body(&signer, &device, &digest, floor);
    let mut downgrade = statement.to_vec();
    downgrade.extend_from_slice(&key.sign(&statement).to_bytes());

    StockRestore {
        manifest: Hex(manifest.try_into().expect("a manifest's length")),
        downgrade: Hex(downgrade.try_into().expect("a statement's length")),
    }
}

/// A release at `version`, signed by `key`, whose image of `len` bytes is
/// the build `build`. The image starts with the bank's marker, as the
/// feeder's sector writer requires.
pub fn signed(key: &SigningKey, version: u32, build: [u8; 32], len: usize) -> Vec<u8> {
    let mut image: Vec<u8> = (0..len)
        .map(|i| u8::try_from(i % 251).unwrap_or_default() ^ build[0])
        .collect();
    let marked = image.len().min(MARKER.len());
    image[..marked].copy_from_slice(&MARKER[..marked]);
    let signed = Signed {
        manifest: Manifest {
            version,
            image_len: u32::try_from(image.len()).expect("a test image fits a bank"),
            target_bank: 1,
            image_sha256: Sha256::digest(&image).into(),
            build_id: build,
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
