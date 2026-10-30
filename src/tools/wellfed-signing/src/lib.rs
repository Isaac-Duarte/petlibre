//! Update signing, only ever of a frozen review: `wellfed-build keygen`
//! and `wellfed-build sign`, and the statements the device API's update
//! path takes (`downgrade`, `revoke`, `add-key`). The bench links it to sign
//! its offers without linking the builder.
//!
//! The firmware accepts any image whose manifest the embedded key signed,
//! so the signer is the last place a reviewed build can be swapped for an
//! unreviewed one. [`Frozen::open`] therefore verifies a review directory
//! exactly as `wellfed-flash` does before it installs one
//! (`wellfed_image::review::frozen`): the image, payload and
//! manifest hashes, the pinned stock hash for a stock review, the first-boot
//! profile and its recorded SDK and toolchain pins (checked against the lock
//! at freeze time, not the checkout's current one), the manifest's
//! `OTA_All.bin`, and the
//! recorded audits. It also requires the image to carry the manifest's
//! startup identity as text. The build ID and the motion policy come from
//! that verified manifest, never from the caller, and [`Frozen::offer`]
//! refuses a motion image unless the caller names the exact policy it is
//! attending.
//!
//! A key is the bench key in `private/ota-key.json` ([`KeyScope::Bench`]),
//! whose public half the firmware builds embed, or an owner's own key
//! ([`KeyScope::Owner`]), which a feeder trusts only once the owner has
//! enrolled it there with a Dispense press. Both are the same file and sign
//! the same manifests. A public release needs an offline release key that
//! never sits in a checkout; this tool does not create or hold one.
//!
//! The manifest bytes are `wellfed_core::ota::offer`'s version 2 format,
//! which the firmware verifies and the device API takes: it names its key
//! and its image kind.
//!
//! A statement binds what it allows: a downgrade names the feeder's identity
//! key, the exact manifest and the floor it was made at; a key statement
//! names the key it retires or adds. Only a key built into the firmware may
//! sign a key statement, and the bench key is the only one built in: an
//! owner's key signs manifests and nothing else a feeder accepts.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use wellfed_core::ota::offer::{self, ImageKind as OfferKind, KeyId, Signed};
use wellfed_core::ota::{self, MARKER, Manifest};
use wellfed_formats::bench::VERIFICATION;
use wellfed_formats::canonical::sha256;
use wellfed_formats::credentials::{KeyScope, OtaKeyFile};
use wellfed_formats::hex;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::private::write_new;
use wellfed_formats::private::{SYSTEM_RANDOM, random};
use wellfed_formats::require;
use wellfed_formats::review::ImageKind;
use wellfed_image::bank::BANK1;
use wellfed_image::ota::STOCK_SHA256;
use wellfed_image::review::frozen;
use wellfed_image::root::Root;

mod error;

pub use error::{Error, FrozenError, ImageError, KeyError, Result};

/// The bench key's root-relative path. Firmware builds embed its public half.
pub const KEY_PATH: &str = "private/ota-key.json";

/// The highest version signed so far in the checkout at `root`, with its
/// file: the bench's manifests in [`VERIFICATION`] and the promotions
/// `wellfed install` kept beside each feeder's backup under `backups`
/// (root-relative unless absolute; `wellfed_formats::bench::STOCK_BACKUPS`
/// by default). The bench
/// and the installer both sign one above it, so neither reuses a version
/// the other signed.
pub fn signed_so_far(root: &Path, backups: &Path) -> Option<(u32, PathBuf)> {
    let mut directories = vec![root.join(VERIFICATION)];
    if let Ok(entries) = std::fs::read_dir(root.join(backups)) {
        directories.extend(entries.flatten().map(|entry| entry.path()));
    }
    directories
        .iter()
        .filter_map(|directory| highest_signed(directory))
        .max_by_key(|(version, _)| *version)
}

/// The highest version among the signed manifests (`*.manifest`) in
/// `directory`, with its file; `None` when it holds none. Only the header
/// is read, unverified: a feeder checks the signature.
pub fn highest_signed(directory: &Path) -> Option<(u32, PathBuf)> {
    std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "manifest"))
        .filter_map(|path| Some((ota::claimed_version(&std::fs::read(&path).ok()?)?, path)))
        .max_by_key(|(version, _)| *version)
}

/// The OTA container's header precedes the bank contents.
pub const CONTAINER_HEADER: usize = 32;

/// Create a new signing key for `scope` at `output`; refuses to replace
/// one. Returns the public key (hex): firmware builds embed a bench key's,
/// and an owner enrolls theirs on each feeder.
pub fn keygen(output: &Path, scope: KeyScope) -> Result<String> {
    let seed: [u8; 32] = random(Path::new(SYSTEM_RANDOM))?;
    let public_key = hex::encode(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());

    let file = OtaKeyFile {
        version: 1,
        scope,
        seed: hex::encode(&seed),
        public_key: public_key.clone(),
    };
    let mut text = serde_json::to_vec_pretty(&file)?;
    text.push(b'\n');
    write_new(output, &text)?;

    Ok(public_key)
}

/// A loaded signing key, and what its file says it is for.
pub struct Key {
    key: SigningKey,
    scope: KeyScope,
}

impl Key {
    /// Load a key file, refusing one whose public key is not its seed's.
    pub fn load(path: &Path) -> Result<Self> {
        let file: OtaKeyFile = wellfed_formats::load(path)?;
        require(file.version == 1, || KeyError::Version {
            version: file.version,
        })?;
        let seed = hex::decode::<32>(&file.seed).ok_or(KeyError::Seed)?;

        let key = SigningKey::from_bytes(&seed);
        require(
            hex::encode(&key.verifying_key().to_bytes()) == file.public_key,
            || KeyError::PublicKey,
        )?;
        Ok(Self {
            key,
            scope: file.scope,
        })
    }

    /// The public half, as the firmware embeds it or an owner enrolls it.
    pub fn public(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// What the key file says the key is for.
    pub fn scope(&self) -> KeyScope {
        self.scope
    }

    /// The id a version 2 manifest names this key by.
    pub fn id(&self) -> KeyId {
        offer::key_id(&self.public())
    }

    fn signed<const N: usize>(&self, body: &[u8; N]) -> Vec<u8> {
        let mut out = body.to_vec();
        out.extend_from_slice(&self.key.sign(body).to_bytes());
        out
    }

    /// A downgrade statement: the feeder whose identity key is `device` may
    /// install `manifest` while its floor is `floor`. Any trusted key signs
    /// one, an owner's enrolled key included: the firmware checks a
    /// downgrade's signer as it checks a manifest's, and the statement is
    /// bound to one feeder, one manifest and one floor.
    pub fn downgrade(&self, manifest: &[u8], device: &[u8; 32], floor: u32) -> Result<Vec<u8>> {
        offer::parse_manifest(manifest).map_err(Error::NotManifest)?;
        let digest = offer::manifest_digest(manifest);
        Ok(self.signed(&offer::downgrade_body(&self.id(), device, &digest, floor)))
    }

    /// A key statement retiring `id` for good. Only the bench key signs
    /// one: a feeder refuses an owner's, so one is never written.
    pub fn revoke(&self, id: &KeyId) -> Result<Vec<u8>> {
        require(self.scope == KeyScope::Bench, || KeyError::OwnerRevoke)?;
        Ok(self.signed(&offer::revoke_body(&self.id(), id)))
    }

    /// A key statement filling the release slot with `public`. Only the
    /// bench key signs one, as for [`Self::revoke`].
    pub fn add_key(&self, public: &[u8; 32]) -> Result<Vec<u8>> {
        require(self.scope == KeyScope::Bench, || KeyError::OwnerAddKey)?;
        Ok(self.signed(&offer::key_add_body(&self.id(), public)))
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
        require(review.join("review.json").is_file(), || {
            FrozenError::NoReview {
                review: review.to_path_buf(),
            }
        })?;
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
                require(contains(payload, identity.as_bytes()), || {
                    FrozenError::NoIdentity {
                        identity: identity.to_owned(),
                    }
                })?;
                build_id
            }
            _ => return Err(FrozenError::KindMismatch.into()),
        };

        Ok(Self {
            image: verified.image,
            kind,
            build_id,
            manifest: verified.manifest,
        })
    }

    /// The pinned stock image as given, without a review directory: an
    /// owner's own backup, for "Restore stock". A stock review binds
    /// nothing but the image, and only the pinned container passes here,
    /// as there; its payload hash names it.
    pub fn stock(image: Vec<u8>) -> Result<Self> {
        require(sha256(&image) == STOCK_SHA256, || FrozenError::NotStock)?;
        let payload = wellfed_image::bank::payload(&image, &BANK1)?;
        let build_id = Sha256::digest(payload).into();
        Ok(Self {
            image,
            kind: ImageKind::Stock,
            build_id,
            manifest: None,
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
        out[body.len()..].copy_from_slice(&key.key.sign(&body).to_bytes());
        Ok(out)
    }

    /// The bank contents a manifest names: the image without its OTA
    /// container's header.
    pub fn payload(&self) -> &[u8] {
        self.image.get(CONTAINER_HEADER..).unwrap_or_default()
    }

    fn attended(&self, motion: Motion) -> Result<()> {
        if let Some(policy) = self.motion_policy() {
            require(motion == Motion::Attended(policy), || {
                FrozenError::Unattended {
                    policy: policy.to_owned(),
                }
            })?;
        }
        Ok(())
    }
}

/// The manifest fields of `container`'s bank contents, refusing what the
/// firmware would refuse.
fn bank_manifest(container: &[u8], version: u32, build_id: [u8; 32]) -> Result<Manifest> {
    require(container.len() > CONTAINER_HEADER + MARKER.len(), || {
        ImageError::TooShort
    })?;
    let payload = &container[CONTAINER_HEADER..];
    require(payload[..MARKER.len()] == MARKER, || ImageError::NoMarker)?;
    let image_len = u32::try_from(payload.len()).map_err(|_| ImageError::TooLarge)?;
    require(
        (ota::SECTOR_BYTES..=ota::BANK1_BYTES).contains(&image_len),
        || ImageError::Size { bytes: image_len },
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
    require(hex::is_lower_hex(text, 64), || FrozenError::BuildId)?;
    hex::decode(text).ok_or_else(|| FrozenError::BuildId.into())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use wellfed_core::ota::SECTOR;
    use wellfed_core::record::Tag;
    use wellfed_formats::bench::STOCK_BACKUPS;

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

    /// A checkout with `private/`, and a frozen build review of `image`
    /// in it.
    fn freeze(image: &[u8], motion: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
        let checkout = tempfile::tempdir().unwrap();
        let review = checkout.path().join("private/review");
        let fields = json!({"startup_identity": BUILD, "startup_motion_policy": motion,
                            "firmware": "feeder"});
        wellfed_image::fixtures::freeze_build(checkout.path(), &review, image, &fields).unwrap();
        (checkout, review)
    }

    fn build_image() -> Vec<u8> {
        wellfed_image::fixtures::image_carrying(
            format!("{} start build={BUILD} bank=", Tag::Start).as_bytes(),
        )
    }

    #[test]
    fn keygen_and_a_frozen_review_sign_what_the_firmware_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ota-key.json");
        let public = keygen(&path, KeyScope::Bench).unwrap();
        assert!(
            keygen(&path, KeyScope::Owner).is_err(),
            "keygen must not replace a key"
        );
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
        let signed = frozen.offer(&key, 9, Motion::Refused).unwrap();

        let parsed = offer::parse_manifest(&signed).unwrap();
        assert_eq!(
            (
                parsed.manifest.version,
                parsed.manifest.image_len as usize,
                hex::encode(&parsed.manifest.build_id),
                parsed.key_id
            ),
            (
                9,
                image.len() - CONTAINER_HEADER,
                BUILD.to_string(),
                key.id()
            )
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

        let key = bench_key(5);
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
            running: None,
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
            admit_statement(&key.revoke(&[7; 8]).unwrap(), &ring),
            Ok(KeyStatement::Revoke([7; 8]))
        );
        assert_eq!(
            admit_statement(&key.add_key(&[8; 32]).unwrap(), &ring),
            Ok(KeyStatement::Add([8; 32]))
        );
    }

    fn bench_key(seed: u8) -> Key {
        Key {
            key: SigningKey::from_bytes(&[seed; 32]),
            scope: KeyScope::Bench,
        }
    }

    /// A key file names its scope: one without it, or whose public key is
    /// not its seed's, is no key.
    #[test]
    fn a_key_file_names_its_scope_and_its_seed_s_public_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.json");
        keygen(&path, KeyScope::Bench).unwrap();
        let mut file: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(file["scope"], json!("bench"));

        let mut unscoped = file.clone();
        unscoped.as_object_mut().unwrap().remove("scope");
        let edited = dir.path().join("edited.json");
        fs::write(&edited, unscoped.to_string()).unwrap();
        assert!(matches!(Key::load(&edited), Err(Error::Formats(_))));

        file["public_key"] = json!("00".repeat(32));
        fs::write(&edited, file.to_string()).unwrap();
        assert!(matches!(
            Key::load(&edited),
            Err(Error::Key(KeyError::PublicKey))
        ));
    }

    /// An owner's key is the same file under its own scope, and signs what
    /// a feeder that enrolled it admits; one that did not refuses it.
    #[test]
    fn an_owner_key_loads_and_signs_for_the_feeders_that_enrolled_it() {
        use wellfed_core::ota::offer::{
            Context, KeyError as RingError, KeyRing, OfferError, Trust,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner-key.json");
        let public = keygen(&path, KeyScope::Owner).unwrap();
        let file: OtaKeyFile = wellfed_formats::load(&path).unwrap();
        assert_eq!(file.scope, KeyScope::Owner);
        let key = Key::load(&path).unwrap();
        assert_eq!(hex::encode(&key.public()), public);

        let (checkout, review) = freeze(&build_image(), None);
        let signed = Frozen::open(checkout.path(), &review)
            .unwrap()
            .offer(&key, 9, Motion::Refused)
            .unwrap();
        const BENCH: [[u8; 32]; 1] = [[0x0b; 32]];
        fn context(enrolled: &[[u8; 32]]) -> Context<'_> {
            Context {
                ring: KeyRing {
                    built_in: &BENCH,
                    added: None,
                    enrolled,
                    revoked: &[],
                },
                floor: 0,
                device: [0x42; 32],
                running: None,
            }
        }
        let admitted = offer::admit_offer(&signed, None, &context(&[key.public()])).unwrap();
        assert_eq!(admitted.trust, Trust::Enrolled);
        assert_eq!(
            offer::admit_offer(&signed, None, &context(&[])),
            Err(OfferError::Key(RingError::Unknown))
        );

        // The firmware takes an enrolled key's downgrade as it takes its
        // manifest, so the owner's key signs one.
        let enrolled = [key.public()];
        let at_floor = Context {
            floor: 9,
            ..context(&enrolled)
        };
        assert_eq!(
            offer::admit_offer(&signed, None, &at_floor),
            Err(OfferError::Downgrade)
        );
        let statement = key.downgrade(&signed, &[0x42; 32], 9).unwrap();
        let allowed = offer::admit_offer(&signed, Some(&statement), &at_floor).unwrap();
        assert_eq!(allowed.downgrade.map(|s| s.trust), Some(Trust::Enrolled));

        // Key statements are the built-in keys' alone: an owner's key
        // writes none.
        assert!(matches!(
            key.revoke(&[7; 8]),
            Err(Error::Key(KeyError::OwnerRevoke))
        ));
        assert!(matches!(
            key.add_key(&[8; 32]),
            Err(Error::Key(KeyError::OwnerAddKey))
        ));

        // A scope the format does not name is no key at all.
        let mut text: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        text["scope"] = json!("release");
        let other = dir.path().join("release.json");
        fs::write(&other, text.to_string()).unwrap();
        assert!(matches!(Key::load(&other), Err(Error::Formats(_))));
    }

    #[test]
    fn signing_refuses_images_the_firmware_would_refuse() {
        let refused = |image: &[u8]| bank_manifest(image, 1, [0; 32]).err();

        let mut no_marker = container(SECTOR, |_| 0);
        no_marker[CONTAINER_HEADER] = b'0';
        assert!(matches!(
            refused(&no_marker),
            Some(Error::Image(ImageError::NoMarker))
        ));
        assert!(matches!(
            refused(&container(100, |_| 0)),
            Some(Error::Image(ImageError::Size { bytes: 100 }))
        ));
        let large = container(ota::BANK1_BYTES as usize + 1, |_| 0);
        assert!(matches!(
            refused(&large),
            Some(Error::Image(ImageError::Size { .. }))
        ));
        assert!(refused(&container(SECTOR, |_| 0)).is_none());
    }

    fn refusal(checkout: &Path, review: &Path) -> Error {
        Frozen::open(checkout, review).unwrap_err()
    }

    #[test]
    fn an_image_that_differs_from_its_review_is_refused() {
        use wellfed_image::review::ReviewError as Review;

        let image = build_image();
        let (checkout, review) = freeze(&image, None);

        // No review.json: nothing says what was reviewed.
        let described = fs::read(review.join("review.json")).unwrap();
        fs::remove_file(review.join("review.json")).unwrap();
        let error = refusal(checkout.path(), &review);
        assert!(
            matches!(&error, Error::Frozen(FrozenError::NoReview { review: path }) if *path == review),
            "{error:?}"
        );
        fs::write(review.join("review.json"), &described).unwrap();

        // A swapped image: a valid container with the same identity.
        let other = wellfed_image::fixtures::image_carrying(
            format!("PLFW1 start build={BUILD} bank=1").as_bytes(),
        );
        fs::write(review.join("image.bin"), &other).unwrap();
        let error = refusal(checkout.path(), &review);
        assert!(
            matches!(
                error,
                Error::Review(wellfed_image::Error::Review(Review::ImageMismatch))
            ),
            "{error:?}"
        );
        fs::write(review.join("image.bin"), &image).unwrap();

        // An edited build ID or motion policy in the manifest.
        let manifest = fs::read_to_string(review.join("build-manifest.json")).unwrap();
        for edited in [
            manifest.replace(BUILD, &"ab".repeat(32)),
            manifest.replace("null", "\"fw16-pulse-v1\""),
        ] {
            fs::write(review.join("build-manifest.json"), edited).unwrap();
            let error = refusal(checkout.path(), &review);
            assert!(
                matches!(
                    error,
                    Error::Review(wellfed_image::Error::Review(Review::ManifestMismatch))
                ),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_review_of_an_image_without_its_identity_is_refused() {
        let image = wellfed_image::fixtures::image_carrying(b"no identity here");
        let (checkout, review) = freeze(&image, None);
        let error = refusal(checkout.path(), &review);
        assert!(
            matches!(&error, Error::Frozen(FrozenError::NoIdentity { identity }) if identity == BUILD),
            "{error:?}"
        );
    }

    #[test]
    fn a_motion_image_needs_its_exact_policy_attended() {
        let key = bench_key(2);
        let (checkout, review) = freeze(&build_image(), Some("fw16-pulse-v1"));
        let frozen = Frozen::open(checkout.path(), &review).unwrap();
        assert_eq!(frozen.motion_policy(), Some("fw16-pulse-v1"));

        for motion in [Motion::Refused, Motion::Attended("fw16-other-v1")] {
            let error = frozen.offer(&key, 1, motion).unwrap_err();
            assert!(
                matches!(&error, Error::Frozen(FrozenError::Unattended { policy }) if policy == "fw16-pulse-v1"),
                "{error:?}"
            );
        }
        assert!(
            frozen
                .offer(&key, 1, Motion::Attended("fw16-pulse-v1"))
                .is_ok()
        );
    }

    /// An owner's backup signs as stock only when it is the pinned stock
    /// container: a build handed in as a backup is refused before anything
    /// is signed.
    #[test]
    fn only_the_pinned_stock_image_signs_without_a_review() {
        let error = Frozen::stock(build_image()).unwrap_err();
        assert!(
            matches!(error, Error::Frozen(FrozenError::NotStock)),
            "{error:?}"
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
        assert!(
            matches!(
                error,
                Error::Review(wellfed_image::Error::Review(
                    wellfed_image::review::ReviewError::NotOriginalStock
                ))
            ),
            "{error:?}"
        );
    }

    /// The next version is picked above every manifest kept in a
    /// directory; anything that is not a manifest is passed over.
    #[test]
    fn the_highest_signed_version_is_read_from_the_headers() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(highest_signed(dir.path()), None);
        assert_eq!(highest_signed(&dir.path().join("missing")), None);

        let manifest = |version: u32| {
            let mut bytes = vec![0u8; offer::MANIFEST_LEN];
            bytes[..8].copy_from_slice(&offer::MAGIC);
            bytes[8..12].copy_from_slice(&version.to_le_bytes());
            bytes
        };
        fs::write(dir.path().join("a-v3.manifest"), manifest(3)).unwrap();
        fs::write(dir.path().join("b-v12.manifest"), manifest(12)).unwrap();
        fs::write(dir.path().join("c.bin"), manifest(40)).unwrap();
        fs::write(dir.path().join("d.manifest"), b"not a manifest").unwrap();

        let (version, path) = highest_signed(dir.path()).unwrap();
        assert_eq!(version, 12);
        assert!(path.ends_with("b-v12.manifest"));
    }

    /// The bench's manifests and the installs' kept promotions are one
    /// sequence: the highest of either is what both sign above.
    #[test]
    fn versions_signed_so_far_cover_the_bench_and_the_installs() {
        let root = tempfile::tempdir().unwrap();
        let backups = Path::new(STOCK_BACKUPS);
        assert_eq!(signed_so_far(root.path(), backups), None);

        let manifest = |version: u32| {
            let mut bytes = vec![0u8; offer::MANIFEST_LEN];
            bytes[..8].copy_from_slice(&offer::MAGIC);
            bytes[8..12].copy_from_slice(&version.to_le_bytes());
            bytes
        };
        let verification = root.path().join(VERIFICATION);
        let feeder = root.path().join(STOCK_BACKUPS).join("ab".repeat(32));
        fs::create_dir_all(&verification).unwrap();
        fs::create_dir_all(&feeder).unwrap();
        fs::write(verification.join("unit-v7.manifest"), manifest(7)).unwrap();
        assert_eq!(signed_so_far(root.path(), backups).unwrap().0, 7);

        fs::write(feeder.join("promotion-v9.manifest"), manifest(9)).unwrap();
        let (version, path) = signed_so_far(root.path(), backups).unwrap();
        assert_eq!(version, 9);
        assert!(path.ends_with("promotion-v9.manifest"));

        // An absolute directory elsewhere counts as given.
        let elsewhere = tempfile::tempdir().unwrap();
        let other = elsewhere.path().join("cd".repeat(32));
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("promotion-v11.manifest"), manifest(11)).unwrap();
        assert_eq!(signed_so_far(root.path(), elsewhere.path()).unwrap().0, 11);
    }
}
