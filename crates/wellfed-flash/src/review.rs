//! Frozen image reviews made by `tools/flash/review-development-image.py`.
//!
//! The review itself stays in Python: it re-runs the build's linked-code and
//! factory-mapping audits under the pinned toolchain and emulator. This side
//! only checks that a frozen review is complete and still matches its image.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::development::{Layout, payload};
use crate::error::{Result, refuse, require};
use crate::evidence::Root;
use wellfed_formats::canonical::sha256;
use wellfed_formats::lock::SdkLock;
use wellfed_formats::manifest::BuildManifest;

pub struct Frozen {
    pub image: Vec<u8>,
    /// SHA-256 of the exact `review.json` bytes, which plans bind.
    pub review_sha256: String,
}

/// `review.json`, as far as a frozen review is checked here. Every field is
/// optional so that a missing one fails the check that needs it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Review {
    schema_version: Option<u64>,
    kind: Option<String>,
    image_kind: Option<String>,
    ota_sha256: Option<String>,
    payload_sha256: Option<String>,
    payload_bytes: Option<u64>,
    /// `null` (or absent) only for the original stock image.
    manifest_sha256: Option<String>,
    review: Option<Details>,
}

/// The audits the Python review re-ran. The startup mains and pad table are
/// compared whole with the manifest's audits, so they stay JSON.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Details {
    startup_mains: Option<Value>,
    pinmap: Option<Value>,
    layout: Option<LayoutReview>,
    factory_mapping: Vec<MappingCase>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LayoutReview {
    sha256: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MappingCase {
    bank: Option<u64>,
}

fn is_size(value: Option<u64>, expected: usize) -> bool {
    value.is_some() && value == u64::try_from(expected).ok()
}

pub fn frozen(root: &Root, directory: &Path, layout: &Layout) -> Result<Frozen> {
    let raw = root.private_file(&directory.join("review.json"))?;
    let review: Review = serde_json::from_slice(&raw)?;
    require(
        review.schema_version == Some(1)
            && review.kind.as_deref() == Some("plaf103_development_image_review"),
        "Unsupported frozen review",
    )?;

    let image = root.private_file(&directory.join("image.bin"))?;
    let data = payload(&image, layout)?;
    require(
        review.ota_sha256.as_deref() == Some(&sha256(&image))
            && review.payload_sha256.as_deref() == Some(&sha256(data))
            && is_size(review.payload_bytes, data.len()),
        "Frozen image/review mismatch",
    )?;

    match review.image_kind.as_deref() {
        Some("stock") => require(
            sha256(&image) == crate::ota::STOCK_SHA256 && review.manifest_sha256.is_none(),
            "Only original stock may omit a build manifest",
        )?,
        Some("build") => build(root, directory, &review, &image)?,
        _ => return refuse("Unknown reviewed image kind"),
    }

    Ok(Frozen {
        image,
        review_sha256: sha256(&raw),
    })
}

fn build(root: &Root, directory: &Path, review: &Review, image: &[u8]) -> Result<()> {
    let manifest_raw = root.private_file(&directory.join("build-manifest.json"))?;
    require(
        review.manifest_sha256.as_deref() == Some(&sha256(&manifest_raw)),
        "Frozen manifest mismatch",
    )?;

    let m: BuildManifest = serde_json::from_slice(&manifest_raw)?;
    let lock: SdkLock = wellfed_formats::load(&root.join("firmware/sdk.lock.json"))?;
    require(
        m.profile == "plaf103-firstboot"
            && m.sdk_revision == lock.sdk.revision
            && m.toolchain_sha256 == lock.toolchain.sha256,
        "Unknown build profile",
    )?;

    let ota = m.artifact("OTA_All.bin");
    require(
        ota.is_some_and(|ota| ota.sha256 == sha256(image) && ota.bytes == image.len() as u64),
        "Manifest/image mismatch",
    )?;

    let unreviewed = Details::default();
    let details = review.review.as_ref().unwrap_or(&unreviewed);
    let mains = details.startup_mains.as_ref();
    // Python's `set(...)` of the audit's per-core mapping: its keys.
    let mut names: Vec<&str> = mains
        .and_then(Value::as_object)
        .map(|cores| cores.keys().map(String::as_str).collect())
        .unwrap_or_default();
    names.sort();
    let startup = m.audits.firstboot_startup_audit.as_ref();
    let audit = m.audits.km0_pinmap_audit.as_ref();
    require(
        mains.is_some_and(|mains| startup.and_then(|s| s.get("mains")) == Some(mains))
            && names == ["hp", "lp"]
            && audit.is_some_and(|audit| {
                details.pinmap.as_ref() == Some(audit) && audit["matches_stock"] == true
            }),
        "Frozen linked/startup/pad review missing",
    )?;

    let banks: Vec<Option<u64>> = details.factory_mapping.iter().map(|c| c.bank).collect();
    require(
        details.layout.as_ref().and_then(|l| l.sha256.as_deref()) == Some(&sha256(image))
            && banks == [Some(0), Some(1)]
            && m.checks.ota_payload_matches
            && m.checks.ota_checksum_matches
            && m.checks.source_snapshot_unchanged
            && m.checks.staged_sources_without_diagnostics,
        "Incomplete build/mapping review",
    )
}
