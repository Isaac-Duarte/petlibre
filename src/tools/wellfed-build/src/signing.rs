//! Update signing, only ever of a frozen review: `wellfed-build keygen`
//! and `wellfed-build sign`, and the statements the device API's update
//! path takes (`downgrade`, `revoke`, `add-key`).
//!
//! The firmware accepts any image whose manifest the embedded key signed,
//! so the signer is the last place a reviewed build can be swapped for an
//! unreviewed one. [`Frozen::open`] therefore verifies a review directory
//! exactly as `wellfed-flash` does before it installs one
//! (`wellfed_flash::evidence::review::frozen`): the image, payload and
//! manifest hashes, the pinned stock hash for a stock review, the first-boot
//! profile against the SDK lock, the manifest's `OTA_All.bin`, and the
//! recorded audits. It also requires the image to carry the manifest's
//! startup identity as text. The build ID and the motion policy come from
//! that verified manifest, never from the caller, and [`Frozen::sign`]
//! refuses a motion image unless the caller names the exact policy it is
//! attending.
//!
//! The key is the bench key in `private/ota-key.json` ([`KeyScope::Bench`]).
//! A public release needs an offline release key that never sits in a
//! checkout; this tool does not create or hold one.
//!
//! The manifest bytes are `wellfed-core::ota`'s formats, which the firmware
//! verifies: tests pin them byte for byte. Version 2
//! (`wellfed_core::ota::offer`) is what the device API takes: it names its
//! key and its image kind. Version 1 stays only for builds that still fetch
//! updates over MQTT, until that path is deleted.
//!
//! A statement binds what it allows: a downgrade names the feeder's identity
//! key, the exact manifest and the floor it was made at; a key statement
//! names the key it retires or adds. Only a key built into the firmware may
//! sign a key statement, and the bench key is the only one built in.

use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::offer::{self, ImageKind as OfferKind, KeyId, Signed};
use wellfed_core::ota::{self, MANIFEST_LEN, MARKER, Manifest};
use wellfed_flash::evidence::review::frozen;
use wellfed_flash::evidence::{Root, write_new};
use wellfed_flash::writers::development::BANK1;
use wellfed_formats::credentials::{KeyScope, OtaKeyFile};
use wellfed_formats::hex;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::private::{SYSTEM_RANDOM, random};
use wellfed_formats::review::ImageKind;
use sha2::{Digest, Sha256};

use crate::{Error, Result, require};

/// The bench key's root-relative path. Firmware builds embed its public half.
pub const KEY_PATH: &str = "private/ota-key.json";

/// The OTA container's header precedes the bank contents.
pub const CONTAINER_HEADER: usize = 32;

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
        // Every scope a key file can name must be decided here: a release
        // key is never one this signer loads from a checkout.
        match file.scope {
            KeyScope::Bench => {}
        }
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

    /// The id a version 2 manifest names this key by.
    pub fn id(&self) -> KeyId {
        offer::key_id(&self.public())
    }

    fn signed<const N: usize>(&self, body: &[u8; N]) -> Vec<u8> {
        let mut out = body.to_vec();
        out.extend_from_slice(&self.0.sign(body).to_bytes());
        out
    }

    /// A downgrade statement: the feeder whose identity key is `device` may
    /// install `manifest` while its floor is `floor`.
    pub fn downgrade(&self, manifest: &[u8], device: &[u8; 32], floor: u32) -> Result<Vec<u8>> {
        offer::parse_manifest(manifest)
            .map_err(|error| Error::Refused(format!("Not a version 2 manifest: {error:?}")))?;
        let digest = offer::manifest_digest(manifest);
        Ok(self.signed(&offer::downgrade_body(&self.id(), device, &digest, floor)))
    }

    /// A key statement retiring `id` for good.
    pub fn revoke(&self, id: &KeyId) -> Vec<u8> {
        self.signed(&offer::revoke_body(&self.id(), id))
    }

    /// A key statement filling the release slot with `public`.
    pub fn add_key(&self, public: &[u8; 32]) -> Vec<u8> {
        self.signed(&offer::key_add_body(&self.id(), public))
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

/// A review directory, verified as `wellfed-flash` verifies one.
#[derive(Debug)]
pub struct Frozen {
    image: Vec<u8>,
    kind: ImageKind,
    build_id: [u8; 32],
    manifest: Option<BuildManifest>,
}

impl Frozen {
    /// Open `review`, a directory under `root`'s `private/`, refusing one
    /// without `review.json` or that differs from what was frozen.
    pub fn open(root: &Path, review: &Path) -> Result<Self> {
        require(
            review.join("review.json").is_file(),
            format!("{} has no review.json", review.display()),
        )?;
        let verified = frozen(&Root::new(root)?, review, &BANK1)?;
        let payload = verified.image.get(CONTAINER_HEADER..).unwrap_or_default();

        let kind = verified.review.image_kind;
        let build_id = match (kind, &verified.manifest) {
            // Stock (the pinned image only) has no startup identity: its
            // payload hash names it, and can never equal a build's identity.
            (ImageKind::Stock, None) => id_bytes(&verified.review.payload_sha256)?,
            (ImageKind::Build, Some(manifest)) => {
                let identity = manifest.startup_identity.as_deref().unwrap_or_default();
                let build_id = id_bytes(identity)?;
                // Every Rust-linked image prints its identity at startup.
                require(
                    contains(payload, identity.as_bytes()),
                    "image.bin does not carry the reviewed build's identity",
                )?;
                build_id
            }
            _ => {
                return Err(Error::Refused(
                    "Frozen review's kind and manifest disagree".into(),
                ));
            }
        };

        Ok(Self {
            image: verified.image,
            kind,
            build_id,
            manifest: verified.manifest,
        })
    }

    /// Stock or a build, as the verified review records it.
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

    /// Sign this image as `version` in the version 1 format that builds
    /// still on MQTT fetch; the result is exactly what they parse.
    pub fn sign(&self, key: &Key, version: u32, motion: Motion) -> Result<[u8; MANIFEST_LEN]> {
        self.attended(motion)?;
        manifest(&key.0, &self.image, version, self.build_id)
    }

    /// Sign this image as `version` for the device API: a version 2
    /// manifest naming `key` and the image's kind.
    pub fn offer(
        &self,
        key: &Key,
        version: u32,
        motion: Motion,
    ) -> Result<[u8; offer::MANIFEST_LEN]> {
        self.attended(motion)?;
        let kind = match self.kind {
            ImageKind::Build => OfferKind::Replacement,
            ImageKind::Stock => OfferKind::Stock,
        };
        let manifest = bank_manifest(&self.image, version, self.build_id)?;
        let signed = Signed {
            manifest,
            kind,
            key_id: key.id(),
        };
        let body = offer::manifest_body(&signed);
        let mut out = [0u8; offer::MANIFEST_LEN];
        out[..body.len()].copy_from_slice(&body);
        out[body.len()..].copy_from_slice(&key.0.sign(&body).to_bytes());
        Ok(out)
    }

    /// The bank contents a manifest names: the image without its OTA
    /// container's header.
    pub fn payload(&self) -> &[u8] {
        self.image.get(CONTAINER_HEADER..).unwrap_or_default()
    }

    fn attended(&self, motion: Motion) -> Result<()> {
        if let Some(policy) = self.motion_policy() {
            require(
                motion == Motion::Attended(policy),
                format!("Image drives the motor ({policy}); sign it only attending that policy"),
            )?;
        }
        Ok(())
    }
}

/// Sign `container`'s bank contents in `wellfed-core::ota`'s version 1
/// format.
fn manifest(
    key: &SigningKey,
    container: &[u8],
    version: u32,
    build_id: [u8; 32],
) -> Result<[u8; MANIFEST_LEN]> {
    let manifest = bank_manifest(container, version, build_id)?;
    let body = ota::manifest_body(&manifest);
    let mut out = [0u8; MANIFEST_LEN];
    out[..body.len()].copy_from_slice(&body);
    out[body.len()..].copy_from_slice(&key.sign(&body).to_bytes());
    Ok(out)
}

/// The manifest fields of `container`'s bank contents, refusing what the
/// firmware would refuse.
fn bank_manifest(container: &[u8], version: u32, build_id: [u8; 32]) -> Result<Manifest> {
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

    Ok(Manifest {
        version,
        image_len,
        target_bank: 1,
        image_sha256: Sha256::digest(payload).into(),
        build_id,
    })
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
    use std::fs;

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

    /// A checkout with `private/`, and a frozen build review of `image`
    /// in it.
    fn freeze(image: &[u8], motion: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
        let checkout = tempfile::tempdir().unwrap();
        let review = checkout.path().join("private/review");
        let fields = json!({"startup_identity": BUILD, "startup_motion_policy": motion,
                            "firmware": "feeder"});
        wellfed_flash::fixtures::freeze_build(checkout.path(), &review, image, &fields).unwrap();
        (checkout, review)
    }

    fn build_image() -> Vec<u8> {
        wellfed_flash::fixtures::image_carrying(
            format!("PLFW1 start build={BUILD} bank=").as_bytes(),
        )
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
        let (checkout, review) = freeze(&image, None);
        let frozen = Frozen::open(checkout.path(), &review).unwrap();
        assert_eq!(
            (frozen.kind(), frozen.build_id()),
            (ImageKind::Build, BUILD.into())
        );
        let signed = frozen.sign(&key, 9, Motion::Refused).unwrap();

        let parsed = ota::parse_manifest(&signed, &key.public(), 8).unwrap();
        assert_eq!(
            (
                parsed.version,
                parsed.image_len as usize,
                hex::encode(&parsed.build_id)
            ),
            (9, image.len() - CONTAINER_HEADER, BUILD.to_string())
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

    /// The device API's manifest and statements, through the firmware's
    /// own checks.
    #[test]
    fn a_version_2_offer_and_its_statements_pass_the_firmware_s_checks() {
        use wellfed_core::ota::offer::{
            Context, KeyRing, KeyStatement, OfferError, Trust, admit_offer, admit_statement,
        };

        let key = Key(SigningKey::from_bytes(&[5; 32]));
        let (checkout, review) = freeze(&build_image(), None);
        let frozen = Frozen::open(checkout.path(), &review).unwrap();
        let signed = frozen.offer(&key, 9, Motion::Refused).unwrap();
        assert_eq!(
            frozen.payload().len(),
            build_image().len() - CONTAINER_HEADER
        );

        let built_in = [key.public()];
        let device = [0x42; 32];
        let context = |floor| Context {
            ring: KeyRing {
                built_in: &built_in,
                added: None,
                enrolled: &[],
                revoked: &[],
            },
            floor,
            device,
            running: [0; 32],
        };
        let admitted = admit_offer(&signed, None, &context(8)).unwrap();
        assert_eq!(admitted.trust, Trust::BuiltIn);
        assert_eq!(admitted.signed.kind, OfferKind::Replacement);
        assert_eq!(hex::encode(&admitted.signed.manifest.build_id), BUILD);

        assert_eq!(
            admit_offer(&signed, None, &context(9)),
            Err(OfferError::Downgrade)
        );
        let statement = key.downgrade(&signed, &device, 9).unwrap();
        assert!(admit_offer(&signed, Some(&statement), &context(9)).is_ok());
        assert!(key.downgrade(&[0; 10], &device, 9).is_err());

        let ring = context(0).ring;
        assert_eq!(
            admit_statement(&key.revoke(&[7; 8]), &ring),
            Ok(KeyStatement::Revoke([7; 8]))
        );
        assert_eq!(
            admit_statement(&key.add_key(&[8; 32]), &ring),
            Ok(KeyStatement::Add([8; 32]))
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

    fn refusal(checkout: &Path, review: &Path) -> String {
        Frozen::open(checkout, review).unwrap_err().to_string()
    }

    #[test]
    fn an_image_that_differs_from_its_review_is_refused() {
        let image = build_image();
        let (checkout, review) = freeze(&image, None);
        let refused = |text: &str| {
            let error = refusal(checkout.path(), &review);
            assert!(error.contains(text), "{error}");
        };

        // No review.json: nothing says what was reviewed.
        let described = fs::read(review.join("review.json")).unwrap();
        fs::remove_file(review.join("review.json")).unwrap();
        refused("no review.json");
        fs::write(review.join("review.json"), &described).unwrap();

        // A swapped image: a valid container with the same identity.
        let other = wellfed_flash::fixtures::image_carrying(
            format!("PLFW1 start build={BUILD} bank=1").as_bytes(),
        );
        fs::write(review.join("image.bin"), &other).unwrap();
        refused("Frozen image/review mismatch");
        fs::write(review.join("image.bin"), &image).unwrap();

        // An edited build ID or motion policy in the manifest.
        let manifest = fs::read_to_string(review.join("build-manifest.json")).unwrap();
        for edited in [
            manifest.replace(BUILD, &"ab".repeat(32)),
            manifest.replace("null", "\"fw16-pulse-v1\""),
        ] {
            fs::write(review.join("build-manifest.json"), edited).unwrap();
            refused("Frozen manifest mismatch");
        }
    }

    #[test]
    fn a_review_of_an_image_without_its_identity_is_refused() {
        let image = wellfed_flash::fixtures::image_carrying(b"no identity here");
        let (checkout, review) = freeze(&image, None);
        assert!(refusal(checkout.path(), &review).contains("identity"));
    }

    #[test]
    fn a_motion_image_needs_its_exact_policy_attended() {
        let key = Key(SigningKey::from_bytes(&[2; 32]));
        let (checkout, review) = freeze(&build_image(), Some("fw16-pulse-v1"));
        let frozen = Frozen::open(checkout.path(), &review).unwrap();
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

    /// A review that calls an image stock signs only the pinned stock
    /// image: otherwise any build, a motion build included, could be
    /// relabelled to skip the motion and health checks.
    #[test]
    fn a_stock_review_of_another_image_is_refused() {
        let image = build_image();
        let (checkout, review) = freeze(&image, Some("fw16-pulse-v1"));
        fs::remove_file(review.join("build-manifest.json")).unwrap();
        let mut frozen: serde_json::Value =
            serde_json::from_slice(&fs::read(review.join("review.json")).unwrap()).unwrap();
        frozen["image_kind"] = json!("stock");
        frozen["manifest_sha256"] = serde_json::Value::Null;
        frozen["review"] = json!({"stock_sha256": frozen["ota_sha256"]});
        fs::write(review.join("review.json"), frozen.to_string()).unwrap();

        let error = refusal(checkout.path(), &review);
        assert!(error.contains("Only original stock"), "{error}");
    }

    /// The pinned stock image, frozen by `wellfed-build review stock`, is
    /// named by its payload hash. Needs the private stock image.
    #[test]
    #[ignore = "needs the pinned stock image under private/"]
    fn the_pinned_stock_review_is_named_by_its_payload_hash() {
        let repo = crate::Repo::here().unwrap();
        let scratch = tempfile::tempdir_in(repo.private()).unwrap();
        let output = scratch.path().join("stock");
        let frozen_review =
            crate::review::freeze(&repo, ImageKind::Stock, Path::new("unused"), &output).unwrap();

        let frozen = Frozen::open(&repo.root, &output).unwrap();
        assert_eq!(
            (frozen.kind(), frozen.build_id()),
            (ImageKind::Stock, frozen_review.payload_sha256)
        );
        let key = Key(SigningKey::from_bytes(&[4; 32]));
        assert!(frozen.sign(&key, 1, Motion::Refused).is_ok());
    }
}
