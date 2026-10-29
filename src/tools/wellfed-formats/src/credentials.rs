//! The private credential and key files: the hotspot's Wi-Fi credentials,
//! the bench update key `wellfed-build keygen` writes and the release key's
//! public half a consumer build embeds, which `wellfed-build` turns into
//! headers and build settings, and an owner's update key
//! `wellfed update-key new` writes, which a feeder trusts only once the
//! owner enrolls it.

use serde::{Deserialize, Serialize};

/// The development Wi-Fi network the first-boot image joins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WifiCredential {
    pub ssid: String,
    pub password: String,
}

/// An update-signing key; `seed` never leaves the file. The bench key and
/// an owner's key share the format: both are an Ed25519 seed that signs
/// the same version 2 manifests, and differ only in how a feeder comes to
/// trust them, which `scope` names.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OtaKeyFile {
    pub version: u32,
    /// What the key may sign.
    pub scope: KeyScope,
    /// Ed25519 seed, hex.
    pub seed: String,
    /// Hex; the firmware build embeds it.
    pub public_key: String,
}

/// What an update-signing key is for.
///
/// Neither is a release key: a public release needs an offline release
/// key first, which never becomes a file here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyScope {
    /// Development images on the bench's own units. It lives in a
    /// checkout's `private/`, and the firmware builds embed its public half.
    Bench,
    /// An owner's own builds, for the feeders that enrolled it as their
    /// update signer (a Dispense press at each feeder confirms it). No
    /// image embeds it, and a feeder that did not enroll it refuses what it
    /// signs.
    Owner,
}

/// The project release key's public half, `private/release-key.pub.json`,
/// which a consumer build embeds as its built-in update key.
///
/// The release key itself is kept offline and never becomes a file in a
/// checkout. Unknown fields are refused, so a file that also carries a
/// `seed` (an [`OtaKeyFile`] copied by mistake) never feeds a build.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseKeyFile {
    /// [`RELEASE_KEY_VERSION`].
    pub version: u32,
    /// Ed25519 public key, 64 lowercase hex characters.
    pub public_key: String,
}

/// The one [`ReleaseKeyFile`] version.
pub const RELEASE_KEY_VERSION: u32 = 1;

/// Where a checkout keeps the [`ReleaseKeyFile`], root-relative.
pub const RELEASE_KEY_PATH: &str = "private/release-key.pub.json";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_key_file_holds_only_the_public_half() {
        let public = "ab".repeat(32);
        let file: ReleaseKeyFile =
            serde_json::from_str(&format!(r#"{{"version": 1, "public_key": "{public}"}}"#))
                .unwrap();
        assert_eq!(file.public_key, public);

        let with_seed = format!(
            r#"{{"version": 1, "seed": "{}", "public_key": "{public}"}}"#,
            "cd".repeat(32)
        );
        assert!(serde_json::from_str::<ReleaseKeyFile>(&with_seed).is_err());
    }
}
