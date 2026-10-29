//! The app's own file in its private storage: how feeders list this phone,
//! and the feeders it owns or uses. The client secret is not here; it
//! lives in secure storage. Written atomically, so a crash keeps the old or
//! the new file, never a torn one.

use std::path::Path;

use serde::{Deserialize, Serialize};
use wellfed_formats::private;

use crate::api::error::{BridgeError, ErrorKind};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppStore {
    /// How feeders list this phone: 1-16 bytes. Empty until setup asks.
    pub label: String,
    pub feeders: Vec<KnownFeeder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownFeeder {
    /// The feeder's identity key, hex: what NSD's `id` matches and the
    /// session pins.
    pub identity: String,
    /// The owner's name for it.
    pub name: String,
    /// Where it last answered (`ip:port`), tried while NSD looks.
    pub address: Option<String>,
}

/// The store at `path`, or an empty one when the app has never saved.
pub fn load_store(path: &str) -> Result<AppStore, BridgeError> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(AppStore::default());
    }
    Ok(wellfed_formats::load(path)?)
}

pub fn save_store(path: &str, store: &AppStore) -> Result<(), BridgeError> {
    private::write_json(Path::new(path), store)?;
    Ok(())
}

/// A key as the store and NSD's `id` spell it: lowercase hex.
#[flutter_rust_bridge::frb(sync)]
pub fn identity_hex(identity: &[u8]) -> String {
    wellfed_formats::hex::encode(identity)
}

/// The first 8 bytes in hex: the fingerprint the feeder logs and the
/// success screen shows.
#[flutter_rust_bridge::frb(sync)]
pub fn fingerprint(identity: &[u8]) -> String {
    wellfed_formats::hex::encode(identity.get(..8).unwrap_or(identity))
}

/// The identity key a store entry names, for opening its session.
#[flutter_rust_bridge::frb(sync)]
pub fn identity_bytes(hex: &str) -> Result<Vec<u8>, BridgeError> {
    wellfed_formats::hex::decode::<32>(hex)
        .map(|key| key.to_vec())
        .ok_or_else(|| BridgeError::new(ErrorKind::Key, "a feeder identity is 64 hex digits"))
}
