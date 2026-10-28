//! A release file: one signed update a client keeps until an owner installs
//! it, whether it came from the project's releases or the owner built it.
//!
//! The file is the version 2 manifest ([`MANIFEST_LEN`] bytes) followed by
//! the image it names: the bank's contents, `image_len` bytes, without the
//! OTA container's header. That is exactly what [`crate::update::Image`]
//! offers, so a verified file is offered as it is.
//!
//! [`verify`] checks the manifest with the feeder's own offer check
//! (`wellfed_core::ota::offer::admit_offer`), not a copy of it, so a file
//! this accepts is one a feeder holding the same keys accepts too: signed by
//! a trusted key, aimed at bank 1, within the bank's size. A client is not a
//! feeder, so the parts of the check that depend on one are fixed: no key is
//! retired, the floor is zero and nothing is running. A stock image, or one
//! at version zero, always needs a downgrade statement bound to one feeder,
//! so it is never a release. The image's length and SHA-256 are then
//! checked against the manifest, which the feeder does only once it has
//! written every sector, and its start against the bank's validity
//! marker, which the feeder's sector writer refuses to write without.

use sha2::{Digest, Sha256};
use wellfed_core::ota::offer::{self, Context, ImageKind, KeyError, KeyRing, Trust};
use wellfed_core::ota::{BANK1_BYTES, MARKER};

pub use wellfed_core::ota::offer::{KeyId, OfferError};

/// The manifest's length, signature included.
pub const MANIFEST_LEN: usize = offer::MANIFEST_LEN;

/// The longest release: a manifest and a full bank.
pub const MAX_LEN: usize = MANIFEST_LEN + BANK1_BYTES as usize;

/// The keys a release may be signed with.
#[derive(Clone, Copy, Debug)]
pub struct Keys<'a> {
    /// The project's keys, which a feeder has built in or in its release
    /// slot.
    pub project: &'a [[u8; 32]],
    /// The owner's update keys, which they enrolled on their feeders.
    pub owner: &'a [[u8; 32]],
}

/// Whose key signed a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signer {
    Project,
    Owner,
}

/// A release that passed every check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified {
    pub version: u32,
    pub image_len: u32,
    pub build_id: [u8; 32],
    pub image_sha256: [u8; 32],
    pub key_id: KeyId,
    pub signer: Signer,
    /// SHA-256 over the manifest, signature included: the name a feeder's
    /// update state and a downgrade statement give it.
    pub digest: [u8; 32],
}

/// Why a file is not a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseError {
    #[error("{len} bytes is shorter than a manifest")]
    Short { len: usize },
    #[error("{len} bytes is longer than a manifest and a full bank")]
    TooLarge { len: usize },
    #[error("the file does not start with a version 2 manifest ({0:?})")]
    Manifest(OfferError),
    #[error("a stock image installs only with a statement for one feeder")]
    Stock,
    #[error("no trusted key has the id the manifest names")]
    UnknownKey,
    #[error("the manifest's signature does not verify")]
    Signature,
    #[error("a feeder would refuse the manifest ({0:?})")]
    Refused(OfferError),
    #[error("the image is {actual} bytes; the manifest names {expected}")]
    ImageLength { expected: u32, actual: usize },
    #[error("the image's SHA-256 is not the one the manifest names")]
    ImageDigest,
    #[error("the image does not start with the bank's validity marker")]
    Marker,
}

/// Check `file` as a release signed by one of `keys`.
pub fn verify(file: &[u8], keys: &Keys<'_>) -> Result<Verified, ReleaseError> {
    let len = file.len();
    if len > MAX_LEN {
        return Err(ReleaseError::TooLarge { len });
    }
    let (Some(manifest), Some(image)) = (file.get(..MANIFEST_LEN), file.get(MANIFEST_LEN..)) else {
        return Err(ReleaseError::Short { len });
    };

    let signed = offer::parse_manifest(manifest).map_err(ReleaseError::Manifest)?;
    if signed.kind == ImageKind::Stock {
        return Err(ReleaseError::Stock);
    }

    let context = Context {
        ring: KeyRing {
            built_in: keys.project,
            added: None,
            enrolled: keys.owner,
            revoked: &[],
        },
        floor: 0,
        device: [0; 32],
        running: None,
    };
    let admitted = offer::admit_offer(manifest, None, &context).map_err(|error| match error {
        OfferError::Key(KeyError::Unknown | KeyError::Revoked) => ReleaseError::UnknownKey,
        OfferError::Signature => ReleaseError::Signature,
        other => ReleaseError::Refused(other),
    })?;

    let m = admitted.signed.manifest;
    if usize::try_from(m.image_len).ok() != Some(image.len()) {
        return Err(ReleaseError::ImageLength {
            expected: m.image_len,
            actual: image.len(),
        });
    }
    if <[u8; 32]>::from(Sha256::digest(image)) != m.image_sha256 {
        return Err(ReleaseError::ImageDigest);
    }
    if !image.starts_with(&MARKER) {
        return Err(ReleaseError::Marker);
    }

    Ok(Verified {
        version: m.version,
        image_len: m.image_len,
        build_id: m.build_id,
        image_sha256: m.image_sha256,
        key_id: admitted.signed.key_id,
        signer: match admitted.trust {
            Trust::Enrolled => Signer::Owner,
            Trust::BuiltIn | Trust::Added => Signer::Project,
        },
        digest: admitted.digest,
    })
}

/// A key's id, as a manifest names its signer: the first 8 bytes of
/// SHA-256 over it.
pub fn key_id(key: &[u8; 32]) -> KeyId {
    offer::key_id(key)
}

/// The manifest and the image of a file [`verify`] accepted.
pub fn parts(file: &[u8]) -> Option<(&[u8], &[u8])> {
    Some((file.get(..MANIFEST_LEN)?, file.get(MANIFEST_LEN..)?))
}
