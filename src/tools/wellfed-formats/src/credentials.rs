//! The private credential and key files the firmware build embeds: the
//! hotspot's Wi-Fi credentials and the update key `wellfed-build keygen`
//! writes; `wellfed-build` turns them into headers.

use serde::{Deserialize, Serialize};

/// The development Wi-Fi network the first-boot image joins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WifiCredential {
    pub ssid: String,
    pub password: String,
}

/// The update-signing key; `seed` never leaves private/.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OtaKeyFile {
    pub version: u32,
    /// What the key may sign. A key file without the field predates it and
    /// is the bench key.
    #[serde(default)]
    pub scope: KeyScope,
    /// Ed25519 seed, hex.
    pub seed: String,
    /// Hex; the firmware build embeds it.
    pub public_key: String,
}

/// What an update-signing key is for.
///
/// Only the bench key exists. It lives in a checkout's `private/`, so it
/// signs development images for the research unit and nothing else; a
/// public release needs an offline release key first, which never becomes
/// a file here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyScope {
    /// Development images on the bench's own units.
    #[default]
    Bench,
}
