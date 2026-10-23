//! Replacement-firmware update signing (FW10). Owner-side only: creates the
//! offline signing key and signs a reviewed image's manifest in the format
//! `wellfed-core::ota` verifies. Nothing here talks to a device.
//!
//! `Offer` is the server side of delivery: one owner-loaded update held only
//! in memory and answered over `ota/get`. Each offer is served to completion
//! once; a manifest request after its last chunk means the candidate fell
//! back, so the offer is dropped until the owner offers it again.
use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::{self, MANIFEST_LEN, MARKER, Manifest};
use wellfed_formats::credentials::OtaKeyFile;
use wellfed_formats::hex;
use sha2::{Digest, Sha256};

use crate::storage;

/// The OTA container's header precedes the bank contents.
pub const CONTAINER_HEADER: usize = 32;

fn unhex<const N: usize>(text: &str) -> Result<[u8; N]> {
    hex::decode(text).with_context(|| format!("expected {N} hex bytes"))
}

/// Create a new signing key; refuses to replace one. Returns the public key
/// (hex), which the firmware build embeds.
pub fn keygen(output: &Path) -> Result<String> {
    let seed: [u8; 32] = storage::random(Path::new(storage::SYSTEM_RANDOM))?;

    let public_key = hex::encode(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
    storage::write_new_json(
        output,
        &OtaKeyFile {
            version: 1,
            seed: hex::encode(&seed),
            public_key: public_key.clone(),
        },
    )?;

    Ok(public_key)
}

pub fn load_key(path: &Path) -> Result<SigningKey> {
    let key: OtaKeyFile = serde_json::from_slice(&fs::read(path)?).context("parse the key file")?;
    ensure!(key.version == 1, "unsupported key file version");
    let signing = SigningKey::from_bytes(&unhex::<32>(&key.seed)?);
    ensure!(
        hex::encode(&signing.verifying_key().to_bytes()) == key.public_key,
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
        (ota::SECTOR_BYTES..=ota::BANK1_BYTES).contains(&image_len),
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

/// One owner-loaded update the server answers `ota/get` from. Checked
/// against its own manifest; authenticity is the firmware's signature check.
#[derive(Clone, Debug)]
pub struct Offer {
    pub manifest: [u8; MANIFEST_LEN],
    pub payload: Vec<u8>,
    pub version: u32,
    /// The last chunk has been served once: a later manifest request means
    /// the candidate fell back and the firmware is starting over.
    pub delivered: bool,
}

impl Offer {
    pub fn new(manifest: &[u8], container: &[u8]) -> Result<Self> {
        let manifest: [u8; MANIFEST_LEN] = manifest
            .try_into()
            .map_err(|_| anyhow::anyhow!("manifest must be {MANIFEST_LEN} bytes"))?;
        ensure!(manifest[..8] == ota::MAGIC, "not an update manifest");

        let payload = self::payload(container)?.to_vec();
        // Offsets follow `ota::manifest_body`: version at 8, image length at 12 and
        // the SHA-256 at 20..52.
        let field = |at: usize| {
            u32::from_le_bytes(
                *manifest[at..]
                    .first_chunk()
                    .expect("inside the checked manifest"),
            )
        };
        ensure!(
            field(12) as usize == payload.len(),
            "manifest length differs from the image"
        );
        ensure!(
            manifest[20..52] == Sha256::digest(&payload)[..],
            "manifest hash differs from the image"
        );

        Ok(Self {
            manifest,
            payload,
            version: field(8),
            delivered: false,
        })
    }

    /// The `ota/data` answer to one `ota/get`, or `None` for a malformed request.
    /// As `answer`, serving each offer to completion once. A manifest request
    /// after the last chunk went out drops the offer: the candidate was
    /// delivered and the firmware fell back (observed on hardware: a failing
    /// build was re-served after every fallback). Re-asks of chunks, such as
    /// a lost last answer, are still served. The owner re-offers to retry.
    /// Returns the answer and whether the offer was dropped.
    pub fn serve(offer: &mut Option<Self>, request: &[u8]) -> (Option<Vec<u8>>, bool) {
        let (part, offset, len) = match ota::decode_get(request) {
            Some(get) => get,
            None => return (None, false),
        };

        let dropped = part == ota::Part::Manifest && offer.as_ref().is_some_and(|o| o.delivered);
        if dropped {
            *offer = None;
        }

        let answer = Self::answer(offer.as_ref(), request);
        // Byte 1 of an answer is its status: only an `Ok` answer for the final
        // chunk counts as delivery.
        if let Some(o) = offer.as_mut()
            && part == ota::Part::Chunk
            && offset as usize + len as usize == o.payload.len()
            && answer.as_ref().is_some_and(|a| a.get(1) == Some(&0))
        {
            o.delivered = true;
        }

        (answer, dropped)
    }

    /// The answer to the normal role's `ota/check`: the manifest, or no
    /// offer. Never counted as a delivery.
    pub fn check(offer: Option<&Self>) -> Vec<u8> {
        Self::answer(offer, &ota::encode_get(ota::Part::Manifest, 0, 0)).unwrap_or_default()
    }

    pub fn answer(offer: Option<&Self>, request: &[u8]) -> Option<Vec<u8>> {
        let (part, offset, len) = ota::decode_get(request)?;
        let refuse = |status| ota::encode_data_header(part, status, offset, 0).to_vec();
        let Some(offer) = offer else {
            return Some(refuse(ota::Status::NoOffer));
        };

        let body: &[u8] = match part {
            ota::Part::Manifest => &offer.manifest,
            ota::Part::Chunk => {
                let end = offset as usize + len as usize;
                if len == 0 || len > ota::MAX_CHUNK || end > offer.payload.len() {
                    return Some(refuse(ota::Status::Range));
                }
                &offer.payload[offset as usize..end]
            }
        };

        let mut out = ota::encode_data_header(
            part,
            ota::Status::Ok,
            offset,
            u32::try_from(body.len()).ok()?,
        )
        .to_vec();
        out.extend_from_slice(body);
        Some(out)
    }
}
