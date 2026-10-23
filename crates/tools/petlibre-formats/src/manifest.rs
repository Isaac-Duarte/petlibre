//! A build's `manifest.json` and the source identity it is bound to.
//!
//! `petlibre-build` writes the manifest; the startup audit, the flash review
//! and the bench read it, and frozen reviews keep a copy as
//! `build-manifest.json`. Every field is always written, as `null` when it
//! does not apply, because `review-development-image.py` compares the audit
//! sections as whole objects.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;
use crate::canonical::{canonical_of, sha256};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildManifest {
    pub profile: String,
    pub sdk_revision: String,
    pub toolchain_sha256: String,
    pub built_at: String,
    pub variant: Option<String>,
    pub ota_public_key: Option<String>,
    /// Root-relative source path to its SHA-256.
    pub inputs: BTreeMap<String, String>,
    pub sdk_overrides: BTreeMap<String, String>,
    pub startup_identity: Option<String>,

    // Startup policies: the audit refuses values it does not know, so they
    // stay strings here rather than enums that would fail the whole load.
    pub startup_output_policy: Option<String>,
    pub startup_led_policy: Option<String>,
    pub startup_gpio_routing_policy: Option<String>,
    pub startup_shared_pad_policy: Option<String>,
    pub startup_gpio_clock_policy: Option<String>,
    pub startup_flash_handshake_policy: Option<String>,
    pub startup_adc_policy: Option<String>,
    pub startup_wifi_policy: Option<String>,
    pub startup_transport_policy: Option<String>,
    pub startup_optics_policy: Option<String>,
    pub startup_motion_policy: Option<String>,
    pub startup_trial_policy: Option<String>,
    pub startup_update_policy: Option<String>,

    pub compiled_archives: Vec<CompiledArchive>,
    pub rust_toolchain: Option<String>,
    pub rust_compiler: Option<RustCompiler>,
    pub toolchain_files: BTreeMap<String, String>,

    #[serde(flatten)]
    pub audits: Audits,

    pub bootloader_replacement_supported: bool,
    pub artifacts: Vec<Artifact>,
    pub checks: BuildChecks,
    pub device_compatibility: String,
}

impl BuildManifest {
    /// The identity the manifest's sources, variant and update key give.
    pub fn source_identity(&self) -> Result<String> {
        IdentitySnapshot {
            profile: &self.profile,
            inputs: &self.inputs,
            variant: self.variant.as_deref(),
            ota_public_key: self.ota_public_key.as_deref(),
        }
        .identity()
    }

    /// The one artifact whose file name is `name`, if exactly one is.
    pub fn artifact(&self, name: &str) -> Option<&Artifact> {
        let mut matching = self
            .artifacts
            .iter()
            .filter(|a| Path::new(&a.path).file_name().is_some_and(|n| n == name));
        let first = matching.next()?;
        matching.next().is_none().then_some(first)
    }
}

/// The linked-image audits the builder records. Their contents belong to
/// each audit (and are compared as objects by the review), so they stay JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audits {
    pub layout_audit: Option<Value>,
    pub km0_pinmap_audit: Option<Value>,
    pub ram_park_audit: Option<Value>,
    pub sdk_hook_audit: Option<Value>,
    pub firstboot_startup_audit: Option<Value>,
    pub wifi_link_audit: Option<Value>,
    pub stack_audit: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Relative to the build output directory.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledArchive {
    /// `KM0` or `KM4`.
    pub core: String,
    pub target: String,
    pub path: String,
    pub sha256: String,
    pub symbols: Vec<String>,
    pub hardware_activated: bool,
}

/// The rustc the build resolved, recorded and compared again afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustCompiler {
    pub version: String,
    pub verbose: Vec<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildChecks {
    pub ota_payload_matches: bool,
    pub ota_checksum_matches: bool,
    pub bringup_banner_present: bool,
    pub required_core_symbols: Vec<String>,
    pub source_snapshot_unchanged: bool,
    pub sdk_worktree_within_allowlist: bool,
    pub staged_sources_without_diagnostics: bool,
}

/// What a build identity binds: SHA-256 of this snapshot's canonical JSON.
/// The variant and update key are left out when absent (not written as
/// `null`), which keeps identities of plain builds what they were before
/// either existed.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct IdentitySnapshot<'a> {
    pub profile: &'a str,
    pub inputs: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ota_public_key: Option<&'a str>,
}

impl IdentitySnapshot<'_> {
    pub fn identity(&self) -> Result<String> {
        Ok(sha256(&canonical_of(self)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::canonical;
    use serde_json::json;

    fn inputs() -> BTreeMap<String, String> {
        BTreeMap::from([("firmware/a.c".to_string(), "00".repeat(32))])
    }

    #[test]
    fn identity_matches_the_untyped_snapshot() {
        let inputs = inputs();
        let snapshot = IdentitySnapshot {
            profile: "plaf103-firstboot",
            inputs: &inputs,
            variant: Some("speaker"),
            ota_public_key: Some("ab"),
        };
        let untyped = json!({"profile": "plaf103-firstboot", "inputs": inputs,
                             "variant": "speaker", "ota_public_key": "ab"});
        assert_eq!(
            snapshot.identity().unwrap(),
            sha256(&canonical(&untyped).unwrap())
        );

        let bare = IdentitySnapshot {
            variant: None,
            ota_public_key: None,
            ..snapshot
        };
        let untyped = json!({"profile": "plaf103-firstboot", "inputs": inputs});
        assert_eq!(
            bare.identity().unwrap(),
            sha256(&canonical(&untyped).unwrap())
        );
    }

    #[test]
    fn manifest_identity_binds_variant_and_key() {
        let manifest = BuildManifest {
            profile: "plaf103-firstboot".into(),
            inputs: inputs(),
            ..Default::default()
        };
        let keyed = BuildManifest {
            ota_public_key: Some("ab".into()),
            ..manifest.clone()
        };
        let variant = BuildManifest {
            variant: Some("speaker".into()),
            ..keyed.clone()
        };
        let ids = [&manifest, &keyed, &variant].map(|m| m.source_identity().unwrap());
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
    }

    #[test]
    fn every_field_is_written_null_when_it_does_not_apply() {
        let written = serde_json::to_value(BuildManifest::default()).unwrap();
        assert_eq!(written["startup_adc_policy"], Value::Null);
        assert_eq!(written["layout_audit"], Value::Null);
    }

    #[test]
    fn artifact_needs_exactly_one_match() {
        let artifact = |path: &str| Artifact {
            path: path.into(),
            ..Default::default()
        };
        let mut manifest = BuildManifest {
            artifacts: vec![artifact("a/OTA_All.bin"), artifact("b/km0.bin")],
            ..Default::default()
        };
        assert_eq!(
            manifest.artifact("OTA_All.bin").unwrap().path,
            "a/OTA_All.bin"
        );
        manifest.artifacts.push(artifact("c/OTA_All.bin"));
        assert!(manifest.artifact("OTA_All.bin").is_none());
    }
}
