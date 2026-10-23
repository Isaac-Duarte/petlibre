//! `firmware/sdk.lock.json` and `firmware/toolchain.lock.json`: the pinned
//! SDK revision, toolchain archive and the toolchain files the build runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkLock {
    pub sdk: SdkPin,
    pub toolchain: ToolchainPin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkPin {
    pub url: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainPin {
    pub url: String,
    pub sha256: String,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum_provenance: Option<String>,
}

/// Per-file hashes of the executables the SDK build and audits invoke, for
/// the archive whose digest `archive_sha256` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainLock {
    pub archive_sha256: String,
    pub files: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_sdk_lock_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../firmware/sdk.lock.json");
        let lock: SdkLock = crate::load(std::path::Path::new(path)).unwrap();
        assert_eq!(lock.sdk.revision.len(), 40);
        assert_eq!(lock.toolchain.sha256.len(), 64);
    }
}
