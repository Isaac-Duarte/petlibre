//! The signer's error type: the key file, the frozen review and the image
//! each refuse with their own enum, wrapped whole here.
//!
//! A refusal is final: nothing here signs a fallback or retries.

use std::path::PathBuf;

use wellfed_core::ota::offer::OfferError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Frozen(#[from] FrozenError),
    #[error(transparent)]
    Image(#[from] ImageError),
    /// A downgrade statement names a manifest the firmware would not parse.
    #[error("Not a version 2 manifest: {0:?}")]
    NotManifest(OfferError),
    /// The frozen review or its image failed the checks the flash client
    /// makes before it installs one.
    #[error(transparent)]
    Review(#[from] wellfed_image::Error),
    #[error(transparent)]
    Formats(#[from] wellfed_formats::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

/// A key file this signer will not load.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("Unsupported key file version")]
    Version { version: u32 },
    #[error("Key file seed is not 32 hex bytes")]
    Seed,
    #[error("Key file's public key does not match its seed")]
    PublicKey,
}

/// A frozen review the image checks accept but the signer does not: what
/// it signs must name the reviewed build, and a motion build its attendant.
#[derive(Debug, thiserror::Error)]
pub enum FrozenError {
    #[error("{} has no review.json", review.display())]
    NoReview { review: PathBuf },
    #[error("Frozen review's kind and manifest disagree")]
    KindMismatch,
    /// An image offered as stock that is not the pinned stock container.
    #[error("Only the pinned stock image signs as stock")]
    NotStock,
    #[error("Frozen review has no 64-hex build ID")]
    BuildId,
    #[error("image.bin does not carry the reviewed build's identity")]
    NoIdentity { identity: String },
    /// A motion policy the caller did not name as the one it attends.
    #[error("Image drives the motor ({policy}); sign it only attending that policy")]
    Unattended { policy: String },
}

/// Bank contents the firmware's own manifest checks would refuse.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("Image too short")]
    TooShort,
    #[error("Image payload lacks the loader marker")]
    NoMarker,
    #[error("Image too large")]
    TooLarge,
    #[error("Image outside the reviewed bank-1 size bound")]
    Size { bytes: u32 },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
