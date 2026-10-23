//! The private credential and key files the firmware build embeds:
//! `petlibre-local` writes them, `petlibre-build` turns them into headers.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// The replacement firmware's broker credential (FW08), as
/// `petlibre-local firmware-credential` writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareCredential {
    pub version: u32,
    pub broker: Ipv4Addr,
    pub port: u16,
    pub device_id: String,
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub namespace: String,
    /// Request-signing key, added by `firmware-request-key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_key: Option<String>,
}

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
    /// Ed25519 seed, hex.
    pub seed: String,
    /// Hex; the firmware build embeds it.
    pub public_key: String,
}
