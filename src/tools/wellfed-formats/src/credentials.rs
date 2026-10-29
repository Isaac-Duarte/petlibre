//! The private credential and key files: the hotspot's Wi-Fi credentials
//! and the bench update key `wellfed-build keygen` writes, which
//! `wellfed-build` turns into headers the firmware embeds, and an owner's
//! update key `wellfed update-key new` writes, which a feeder trusts only
//! once the owner enrolls it.

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
