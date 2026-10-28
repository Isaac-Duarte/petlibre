//! The `[releases]` settings: whether the project's releases are fetched,
//! and the update keys a release may be signed with.
//!
//! The keys are settings rather than built in: the project has no public
//! release key yet (the firmware's built-in key is the bench key, and an
//! offline release key reaches a feeder through its release slot), so a
//! server trusts exactly the keys its owner lists, and a fresh one trusts
//! none and keeps no release.

use std::str::FromStr;

use serde::Deserialize;
use serde::de::IntoDeserializer as _;
use serde::de::value::StrDeserializer;
use wellfed_api::Hex;

use super::{InvalidValue, given};

/// The release cache, in the data directory.
pub const RELEASES: &str = "releases";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Releases {
    /// Whether the project's GitHub releases are checked daily. Uploads work
    /// either way, and nothing installs without an owner asking.
    pub fetch: bool,
    /// The project's update keys: the key built into the feeders' firmware,
    /// or the release key their release slot holds.
    pub project_keys: Vec<UpdateKey>,
    /// The owner's update keys, as they enrolled them on their feeders: a
    /// release one of them signed (a self-built image) is kept beside the
    /// project's.
    pub owner_keys: Vec<UpdateKey>,
}

impl Releases {
    pub(super) fn merge(file: ReleasesFile, overrides: ReleaseOverrides) -> Self {
        Self {
            fetch: overrides.releases_fetch.or(file.fetch).unwrap_or(false),
            project_keys: given(overrides.release_project_keys)
                .or(file.project_keys)
                .unwrap_or_default(),
            owner_keys: given(overrides.release_owner_keys)
                .or(file.owner_keys)
                .unwrap_or_default(),
        }
    }
}

/// The release flags, each with its environment variable.
#[derive(Debug, Default, clap::Args)]
pub struct ReleaseOverrides {
    /// Whether GitHub releases are checked daily (`releases.fetch`; default
    /// false).
    #[arg(long, env = "WELLFED_SERVER_RELEASES_FETCH", value_name = "BOOL")]
    pub releases_fetch: Option<bool>,
    /// A project update key, in hex (`releases.project_keys`). Repeat for
    /// more.
    #[arg(
        long = "release-project-key",
        env = "WELLFED_SERVER_RELEASE_PROJECT_KEYS",
        value_delimiter = ',',
        value_name = "HEX"
    )]
    pub release_project_keys: Vec<UpdateKey>,
    /// An owner's update key, in hex (`releases.owner_keys`). Repeat for
    /// more.
    #[arg(
        long = "release-owner-key",
        env = "WELLFED_SERVER_RELEASE_OWNER_KEYS",
        value_delimiter = ',',
        value_name = "HEX"
    )]
    pub release_owner_keys: Vec<UpdateKey>,
}

/// `[releases]` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReleasesFile {
    fetch: Option<bool>,
    project_keys: Option<Vec<UpdateKey>>,
    owner_keys: Option<Vec<UpdateKey>>,
}

/// An Ed25519 update-signing public key, as 64 lowercase hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct UpdateKey(pub [u8; 32]);

impl TryFrom<String> for UpdateKey {
    type Error = InvalidValue;

    fn try_from(text: String) -> Result<Self, InvalidValue> {
        let hex: StrDeserializer<'_, serde::de::value::Error> = text.as_str().into_deserializer();
        match Hex::<32>::deserialize(hex) {
            Ok(Hex(key)) => Ok(Self(key)),
            Err(_) => Err(InvalidValue::UpdateKey { text }),
        }
    }
}

impl FromStr for UpdateKey {
    type Err = InvalidValue;

    fn from_str(text: &str) -> Result<Self, InvalidValue> {
        Self::try_from(text.to_owned())
    }
}
