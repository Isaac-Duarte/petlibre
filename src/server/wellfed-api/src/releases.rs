//! The releases the server keeps for its owner to install: fetched from the
//! project's GitHub releases or uploaded, and verified the same way.

use jiff::Timestamp;
use wellfed_model::Hex;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// A verified release in the server's cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Release {
    /// SHA-256 over the signed manifest: the name a feeder's update state
    /// gives the install.
    #[schema(value_type = String)]
    pub id: Hex<32>,
    pub version: u32,
    #[schema(value_type = String)]
    pub build_id: Hex<32>,
    /// The image's length in bytes.
    pub image_len: u32,
    #[schema(value_type = String)]
    pub image_sha256: Hex<32>,
    /// The signing key's id: the first 8 bytes of SHA-256 over it.
    #[schema(value_type = String)]
    pub key_id: Hex<8>,
    pub signer: ReleaseSigner,
    pub source: ReleaseSource,
    /// When the server took it in.
    pub added: Timestamp,
}

/// Whose key signed a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseSigner {
    /// The project's key.
    Project,
    /// The owner's enrolled update key: a self-built image.
    Owner,
}

/// Where a release came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReleaseSource {
    /// An asset of one of the project's GitHub releases.
    Github { tag: String, asset: String },
    /// A file an owner uploaded.
    Upload,
}

/// A page of releases, newest version first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReleasePage {
    pub items: Vec<Release>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReleaseQuery {
    /// The `next` of the previous page.
    pub after: Option<String>,
    /// At most this many (1-1000, default 100).
    pub limit: Option<u32>,
}

/// What one check of the project's releases found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReleaseCheck {
    /// Releases verified and added to the cache by this check.
    pub added: Vec<Release>,
    /// Assets that failed verification. Each is skipped by later checks
    /// until the server restarts.
    pub refused: Vec<RefusedAsset>,
    /// Assets that failed to download. The next check tries them again.
    pub failed: Vec<AssetName>,
}

/// One asset of one of the project's releases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AssetName {
    pub tag: String,
    pub asset: String,
}

/// A release asset the server would not keep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RefusedAsset {
    pub tag: String,
    pub asset: String,
    pub reason: ReleaseRefusal,
}

/// Why a file is not a release the server keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRefusal {
    /// Shorter than a manifest, or longer than a manifest and a full bank.
    Size,
    /// It does not start with a version 2 manifest.
    Manifest,
    /// A stock image, or one at version 0: each installs only with a
    /// statement made for one feeder.
    NeedsStatement,
    /// Signed by a key that is neither the project's nor the owner's.
    UnknownKey,
    /// The manifest's signature does not verify.
    Signature,
    /// A feeder would refuse the manifest: another bank, or an image too
    /// small or too large for bank 1.
    Refused,
    /// The image is not the one the manifest names (its length or its
    /// SHA-256 differ), or it lacks the bank's validity marker.
    Image,
}
