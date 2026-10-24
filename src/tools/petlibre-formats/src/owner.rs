//! The owner's client key for the feeder's protocols: the `petlibre`
//! CLI creates it at its first setup and records each feeder that enrolled
//! it; the sessions read it. The secret never leaves `private/`.

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerKeyFile {
    pub version: u32,
    /// The client's X25519 static secret, hex.
    pub secret: String,
    /// How feeders list this client: 1-16 bytes.
    pub label: String,
    /// The feeders that enrolled this key.
    #[serde(default)]
    pub feeders: Vec<EnrolledFeeder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrolledFeeder {
    /// The feeder's X25519 identity public key, hex. Its first 8 bytes are
    /// the fingerprint the feeder logs (`PLID1`).
    pub identity: String,
}
