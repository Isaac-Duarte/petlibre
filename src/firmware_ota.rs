//! Replacement-firmware update signing (FW10). Owner-side only: creates the
//! offline signing key and signs a reviewed image's manifest in the format
//! `wellfed-core::ota` verifies. Nothing here talks to a device.
use std::{fs, io::Read, path::Path};

use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::{self, MANIFEST_LEN, MARKER, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage;

/// The OTA container's header precedes the bank contents.
pub const CONTAINER_HEADER: usize = 32;

#[derive(Debug, Deserialize, Serialize)]
pub struct KeyFile {
    pub version: u32,
    /// Ed25519 seed; never leaves private/.
    pub seed: String,
    pub public_key: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(text: &str) -> Result<[u8; N]> {
    ensure!(
        text.len() == 2 * N && text.is_ascii(),
        "expected {N} hex bytes"
    );
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).context("invalid hex")?;
    }
    Ok(out)
}

/// Create a new signing key; refuses to replace one. Returns the public key
/// (hex), which the firmware build embeds.
pub fn keygen(output: &Path) -> Result<String> {
    let mut seed = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut seed))
        .context("read the system random source")?;
    let public_key = hex(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
    storage::write_new_json(
        output,
        &KeyFile {
            version: 1,
            seed: hex(&seed),
            public_key: public_key.clone(),
        },
    )?;
    Ok(public_key)
}

pub fn load_key(path: &Path) -> Result<SigningKey> {
    let key: KeyFile = serde_json::from_slice(&fs::read(path)?).context("parse the key file")?;
    ensure!(key.version == 1, "unsupported key file version");
    let signing = SigningKey::from_bytes(&unhex::<32>(&key.seed)?);
    ensure!(
        hex(&signing.verifying_key().to_bytes()) == key.public_key,
        "key file's public key does not match its seed"
    );
    Ok(signing)
}

/// The bank contents of an OTA container: everything after its header,
/// which must start with the loader's marker.
pub fn payload(container: &[u8]) -> Result<&[u8]> {
    ensure!(
        container.len() > CONTAINER_HEADER + MARKER.len(),
        "image too short"
    );
    let payload = &container[CONTAINER_HEADER..];
    ensure!(
        payload[..MARKER.len()] == MARKER,
        "image payload lacks the loader marker"
    );
    Ok(payload)
}

/// Sign a manifest for `container`; the result is exactly what the firmware parses.
pub fn sign(
    key: &SigningKey,
    container: &[u8],
    version: u32,
    build_id: [u8; 32],
) -> Result<[u8; MANIFEST_LEN]> {
    let payload = payload(container)?;
    let image_len = u32::try_from(payload.len()).context("image too large")?;
    ensure!(
        (ota::SECTOR as u32..=ota::BANK1_BYTES).contains(&image_len),
        "image outside the reviewed bank-1 size bound"
    );
    let manifest = Manifest {
        version,
        image_len,
        target_bank: 1,
        image_sha256: Sha256::digest(payload).into(),
        build_id,
    };
    let body = ota::manifest_body(&manifest);
    let mut out = [0u8; MANIFEST_LEN];
    out[..body.len()].copy_from_slice(&body);
    out[body.len()..].copy_from_slice(&key.sign(&body).to_bytes());
    Ok(out)
}

pub fn parse_build_id(text: &str) -> Result<[u8; 32]> {
    unhex::<32>(text)
}
