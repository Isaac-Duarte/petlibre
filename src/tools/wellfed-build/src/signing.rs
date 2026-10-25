//! Update signing, only ever of a frozen review: `wellfed-build keygen`
//! and `wellfed-build sign`.
//!
//! The firmware accepts any image whose manifest the embedded key signed,
//! so the signer is the last place a reviewed build can be swapped for an
//! unreviewed one. [`Frozen::open`] therefore re-checks a review directory
//! as `wellfed-build review` left it: `image.bin` must hash to the review's
//! `ota_sha256`, a build's `build-manifest.json` must hash to its
//! `manifest_sha256`, and the image must carry the startup identity that
//! manifest records. The build ID and the motion policy come from that
//! frozen manifest, never from the caller, and [`Frozen::sign`] refuses a
//! motion image unless the caller names the exact policy it is attending.
//!
//! The key is the bench key in `private/ota-key.json` ([`KeyScope::Bench`]).
//! A public release needs an offline release key that never sits in a
//! checkout; this tool does not create or hold one.
//!
//! The manifest bytes are `wellfed-core::ota`'s format, which the firmware
//! verifies: a test pins them byte for byte.

use std::fs;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::{self, MANIFEST_LEN, MARKER, Manifest};
use wellfed_flash::evidence::write_new;
use wellfed_formats::canonical::sha256;
use wellfed_formats::credentials::{KeyScope, OtaKeyFile};
use wellfed_formats::hex;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::private::{SYSTEM_RANDOM, random};
use wellfed_formats::review::{self, Details, ImageKind, ImageReview};
use sha2::{Digest, Sha256};

use crate::{Error, Result, require};

/// The bench key's root-relative path. Firmware builds embed its public half.
pub const KEY_PATH: &str = "private/ota-key.json";

/// The OTA container's header precedes the bank contents.
const CONTAINER_HEADER: usize = 32;

/// Create a new bench signing key at `output`; refuses to replace one.
/// Returns the public key (hex), which firmware builds embed.
pub fn keygen(output: &Path) -> Result<String> {
    let seed: [u8; 32] = random(Path::new(SYSTEM_RANDOM))?;
    let public_key = hex::encode(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());

    let file = OtaKeyFile {
        version: 1,
        scope: KeyScope::Bench,
        seed: hex::encode(&seed),
        public_key: public_key.clone(),
    };
    let mut text = serde_json::to_vec_pretty(&file)?;
    text.push(b'\n');
    write_new(output, &text)?;

    Ok(public_key)
}

/// A loaded signing key.
pub struct Key(SigningKey);

impl Key {
    /// Load a key file, refusing one whose public key is not its seed's.
    pub fn load(path: &Path) -> Result<Self> {
        let file: OtaKeyFile = wellfed_formats::load(path)?;
        require(file.version == 1, "Unsupported key file version")?;
        let seed = hex::decode::<32>(&file.seed)
            .ok_or_else(|| Error::Refused("Key file seed is not 32 hex bytes".into()))?;

        let key = SigningKey::from_bytes(&seed);
        require(
            hex::encode(&key.verifying_key().to_bytes()) == file.public_key,
            "Key file's public key does not match its seed",
        )?;
        Ok(Self(key))
    }

    /// The public half, as the firmware embeds it.
    pub fn public(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
}

/// Whoever signs a motion image is attending it, naming the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion<'a> {
    /// Sign only an image that moves nothing at startup.
    Refused,
    /// Also sign an image with exactly this startup motion policy.
    Attended(&'a str),
}

/// A review directory, re-checked against its own `review.json`.
#[derive(Debug)]
pub struct Frozen {
    image: Vec<u8>,
    kind: ImageKind,
    build_id: [u8; 32],
    manifest: Option<BuildManifest>,
}

impl Frozen {
    /// Open `review`, refusing a directory without `review.json` and an
    /// `image.bin` or `build-manifest.json` that differs from it.
    pub fn open(review: &Path) -> Result<Self> {
        let described = review.join("review.json");
        require(
            described.is_file(),
            format!("{} has no review.json", review.display()),
        )?;
        let frozen: ImageReview = wellfed_formats::load(&described)?;
        require(
            frozen.schema_version == review::SCHEMA_VERSION && frozen.kind == review::KIND,
            "Unsupported review.json",
        )?;

        let image = fs::read(review.join("image.bin"))?;
        require(
            sha256(&image) == frozen.ota_sha256,
            "image.bin differs from its frozen review",
        )?;
        let payload = image.get(CONTAINER_HEADER..).unwrap_or_default();
        require(
            sha256(payload) == frozen.payload_sha256,
            "image.bin's payload differs from its frozen review",
        )?;

        let (build_id, manifest) = match (frozen.image_kind, &frozen.review) {
            // Stock has no startup identity: its payload hash names it, and
            // can never equal a replacement build's identity.
            (ImageKind::Stock, Details::Stock { stock_sha256 }) => {
                require(
                    *stock_sha256 == frozen.ota_sha256,
                    "Stock review's image hash differs from its stock hash",
                )?;
                (id_bytes(&frozen.payload_sha256)?, None)
            }
            (ImageKind::Build, Details::Build(_)) => {
                let raw = fs::read(review.join("build-manifest.json"))?;
                require(
                    frozen.manifest_sha256.as_deref() == Some(sha256(&raw).as_str()),
                    "build-manifest.json differs from its frozen review",
                )?;
                let manifest: BuildManifest = serde_json::from_slice(&raw)?;
                let identity = manifest.startup_identity.as_deref().unwrap_or_default();
                let build_id = id_bytes(identity)?;
                // Every Rust-linked image prints its identity at startup, so
                // the reviewed identity is in the image as text.
                require(
                    contains(payload, identity.as_bytes()),
                    "image.bin does not carry the reviewed build's identity",
                )?;
                (build_id, Some(manifest))
            }
            _ => {
                return Err(Error::Refused(
                    "review.json's details do not match its image kind".into(),
                ));
            }
        };

        Ok(Self {
            image,
            kind: frozen.image_kind,
            build_id,
            manifest,
        })
    }

    pub fn kind(&self) -> ImageKind {
        self.kind
    }

    /// The build ID the manifest will carry, as lowercase hex.
    pub fn build_id(&self) -> String {
        hex::encode(&self.build_id)
    }

    /// A build's frozen manifest; `None` for stock.
    pub fn manifest(&self) -> Option<&BuildManifest> {
        self.manifest.as_ref()
    }

    /// The startup motion policy the frozen build declares, if it moves.
    pub fn motion_policy(&self) -> Option<&str> {
        self.manifest
            .as_ref()
            .and_then(|m| m.startup_motion_policy.as_deref())
            .filter(|p| !p.is_empty())
    }

    /// Sign this image as `version`; the result is exactly what the
    /// firmware parses.
    pub fn sign(&self, key: &Key, version: u32, motion: Motion) -> Result<[u8; MANIFEST_LEN]> {
        if let Some(policy) = self.motion_policy() {
            require(
                motion == Motion::Attended(policy),
                format!("Image drives the motor ({policy}); sign it only attending that policy"),
            )?;
        }
        manifest(&key.0, &self.image, version, self.build_id)
    }
}

/// Sign `container`'s bank contents in `wellfed-core::ota`'s format.
fn manifest(
    key: &SigningKey,
    container: &[u8],
    version: u32,
    build_id: [u8; 32],
) -> Result<[u8; MANIFEST_LEN]> {
    require(
        container.len() > CONTAINER_HEADER + MARKER.len(),
        "Image too short",
    )?;
    let payload = &container[CONTAINER_HEADER..];
    require(
        payload[..MARKER.len()] == MARKER,
        "Image payload lacks the loader marker",
    )?;
    let image_len =
        u32::try_from(payload.len()).map_err(|_| Error::Refused("Image too large".into()))?;
    require(
        (ota::SECTOR_BYTES..=ota::BANK1_BYTES).contains(&image_len),
        "Image outside the reviewed bank-1 size bound",
    )?;

    let body = ota::manifest_body(&Manifest {
        version,
        image_len,
        target_bank: 1,
        image_sha256: Sha256::digest(payload).into(),
        build_id,
    });
    let mut out = [0u8; MANIFEST_LEN];
    out[..body.len()].copy_from_slice(&body);
    out[body.len()..].copy_from_slice(&key.sign(&body).to_bytes());
    Ok(out)
}

fn id_bytes(text: &str) -> Result<[u8; 32]> {
    require(
        hex::is_lower_hex(text, 64),
        "Frozen review has no 64-hex build ID",
    )?;
    hex::decode(text).ok_or_else(|| Error::Refused("Frozen review has no 64-hex build ID".into()))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use wellfed_core::ota::{ManifestError, SECTOR};
    use serde_json::json;

    use super::*;

    const BUILD: &str = "8a13e59efa15e56b5f5e32825909dc45e381a2027abc65d8de6658c40b302447";

    /// An OTA container of `len` payload bytes starting with the marker.
    fn container(len: usize, fill: impl Fn(usize) -> u8) -> Vec<u8> {
        let mut image = vec![0u8; CONTAINER_HEADER];
        let mut payload: Vec<u8> = (0..len).map(fill).collect();
        payload[..8].copy_from_slice(&MARKER);
        image.extend(payload);
        image
    }

    /// What `wellfed-local ota-sign` wrote for these inputs before signing
    /// moved here; the firmware verifies these bytes, so they never change.
    const GOLDEN: &str = "504c4f5441310000070000002c21000001000000bf4259250978564e89539ff7aa5f9ca70b8a6a2a1a332c7ac525a1e1ae8480c1abababababababababababababababababababababababababababababababab15161bed48be30e55ebb3b0b76bd533fce5dc04d2b82bd7615865a06de8699753f082841869bb398becaa1f251e4313c4b500ba5b7045ac05228df7ab3681509";

    #[test]
    fn manifest_bytes_match_the_former_signer() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let image = container(2 * SECTOR + 300, |i| (i * 7 % 251) as u8);

        let signed = manifest(&key, &image, 7, [0xab; 32]).unwrap();
        assert_eq!(hex::encode(&signed), GOLDEN);
    }

    /// A frozen build review of `image` in a new directory under `parent`.
    fn freeze(parent: &Path, image: &[u8], motion: Option<&str>) -> std::path::PathBuf {
        let review = parent.join("review");
        fs::create_dir(&review).unwrap();
        let manifest = json!({"startup_identity": BUILD, "startup_motion_policy": motion,
                              "firmware": "feeder"})
        .to_string();
        fs::write(review.join("image.bin"), image).unwrap();
        fs::write(review.join("build-manifest.json"), &manifest).unwrap();
        let frozen = json!({
            "schema_version": 1,
            "kind": review::KIND,
            "image_kind": "build",
            "ota_sha256": sha256(image),
            "payload_sha256": sha256(&image[CONTAINER_HEADER..]),
            "payload_bytes": image.len() - CONTAINER_HEADER,
            "manifest_sha256": sha256(manifest.as_bytes()),
            "review": {"startup_mains": null, "layout": null, "pinmap": null,
                       "factory_mapping": [], "scope": "test"},
            "reviewer_sha256": "3".repeat(64),
            "application_boot_verified": false
        });
        fs::write(review.join("review.json"), frozen.to_string()).unwrap();
        review
    }

    fn build_image() -> Vec<u8> {
        let mut image = container(SECTOR + 700, |i| (i % 253) as u8);
        let text = format!("PLFW1 start build={BUILD} bank=");
        image[100..100 + text.len()].copy_from_slice(text.as_bytes());
        image
    }

    #[test]
    fn keygen_and_a_frozen_review_sign_what_the_firmware_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ota-key.json");
        let public = keygen(&path).unwrap();
        assert!(keygen(&path).is_err(), "keygen must not replace a key");
        let file: OtaKeyFile = wellfed_formats::load(&path).unwrap();
        assert_eq!(file.scope, KeyScope::Bench);
        let key = Key::load(&path).unwrap();
        assert_eq!(hex::encode(&key.public()), public);

        let image = build_image();
        let frozen = Frozen::open(&freeze(dir.path(), &image, None)).unwrap();
        assert_eq!(frozen.build_id(), BUILD);
        let signed = frozen.sign(&key, 9, Motion::Refused).unwrap();

        let parsed = ota::parse_manifest(&signed, &key.public(), 8).unwrap();
        assert_eq!(
            (
                parsed.version,
                parsed.image_len,
                hex::encode(&parsed.build_id)
            ),
            (9, (SECTOR + 700) as u32, BUILD.to_string())
        );
        assert_eq!(
            ota::parse_manifest(&signed, &key.public(), 9),
            Err(ManifestError::Downgrade)
        );

        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_key_file_without_a_scope_is_the_bench_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.json");
        keygen(&path).unwrap();
        let mut file: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        file.as_object_mut().unwrap().remove("scope");
        let old = dir.path().join("old.json");
        fs::write(&old, file.to_string()).unwrap();
        assert_eq!(
            wellfed_formats::load::<OtaKeyFile>(&old).unwrap().scope,
            KeyScope::Bench
        );
        assert!(Key::load(&old).is_ok());

        file["public_key"] = json!("00".repeat(32));
        fs::write(&old, file.to_string()).unwrap();
        assert!(Key::load(&old).is_err());
    }

    #[test]
    fn signing_refuses_images_the_firmware_would_refuse() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let mut no_marker = container(SECTOR, |_| 0);
        no_marker[CONTAINER_HEADER] = b'0';
        assert!(manifest(&key, &no_marker, 1, [0; 32]).is_err());
        assert!(manifest(&key, &container(100, |_| 0), 1, [0; 32]).is_err());
        let large = container(ota::BANK1_BYTES as usize + 1, |_| 0);
        assert!(manifest(&key, &large, 1, [0; 32]).is_err());
    }

    fn refusal(review: &Path) -> String {
        Frozen::open(review).unwrap_err().to_string()
    }

    #[test]
    fn an_image_that_differs_from_its_review_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let image = build_image();
        let review = freeze(dir.path(), &image, None);

        // No review.json: nothing says what was reviewed.
        let described = fs::read(review.join("review.json")).unwrap();
        fs::remove_file(review.join("review.json")).unwrap();
        assert!(refusal(&review).contains("no review.json"));
        fs::write(review.join("review.json"), &described).unwrap();

        // A swapped image.
        let mut other = image.clone();
        other[CONTAINER_HEADER + 9] ^= 1;
        fs::write(review.join("image.bin"), &other).unwrap();
        assert!(refusal(&review).contains("image.bin differs"));
        fs::write(review.join("image.bin"), &image).unwrap();

        // An edited build ID or motion policy in the manifest.
        let manifest = fs::read_to_string(review.join("build-manifest.json")).unwrap();
        for edited in [
            manifest.replace(BUILD, &"ab".repeat(32)),
            manifest.replace("null", "\"fw16-pulse-v1\""),
        ] {
            fs::write(review.join("build-manifest.json"), edited).unwrap();
            assert!(refusal(&review).contains("build-manifest.json differs"));
        }
    }

    #[test]
    fn a_review_of_an_image_without_its_identity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let image = container(SECTOR, |_| 0);
        let review = freeze(dir.path(), &image, None);
        assert!(refusal(&review).contains("identity"));
    }

    #[test]
    fn a_motion_image_needs_its_exact_policy_attended() {
        let dir = tempfile::tempdir().unwrap();
        let key = Key(SigningKey::from_bytes(&[2; 32]));
        let review = freeze(dir.path(), &build_image(), Some("fw16-pulse-v1"));
        let frozen = Frozen::open(&review).unwrap();
        assert_eq!(frozen.motion_policy(), Some("fw16-pulse-v1"));

        for motion in [Motion::Refused, Motion::Attended("fw16-other-v1")] {
            let error = frozen.sign(&key, 1, motion).unwrap_err().to_string();
            assert!(error.contains("drives the motor"), "{error}");
        }
        assert!(
            frozen
                .sign(&key, 1, Motion::Attended("fw16-pulse-v1"))
                .is_ok()
        );
    }

    #[test]
    fn a_stock_review_is_named_by_its_payload_hash() {
        let dir = tempfile::tempdir().unwrap();
        let review = dir.path().join("stock");
        fs::create_dir(&review).unwrap();
        let image = container(SECTOR, |i| (i % 7) as u8);
        fs::write(review.join("image.bin"), &image).unwrap();
        let payload = sha256(&image[CONTAINER_HEADER..]);
        let frozen = json!({
            "schema_version": 1, "kind": review::KIND, "image_kind": "stock",
            "ota_sha256": sha256(&image), "payload_sha256": payload,
            "payload_bytes": SECTOR, "manifest_sha256": null,
            "review": {"stock_sha256": sha256(&image)},
            "reviewer_sha256": "3".repeat(64), "application_boot_verified": false
        });
        fs::write(review.join("review.json"), frozen.to_string()).unwrap();

        let frozen = Frozen::open(&review).unwrap();
        assert_eq!(
            (frozen.kind(), frozen.build_id()),
            (ImageKind::Stock, payload)
        );
    }
}
