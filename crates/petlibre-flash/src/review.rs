//! Frozen image reviews made by `tools/flash/review-development-image.py`.
//!
//! The review itself stays in Python: it re-runs the build's linked-code and
//! factory-mapping audits under the pinned toolchain and emulator. This side
//! only checks that a frozen review is complete and still matches its image.

use std::path::Path;

use serde_json::Value;

use crate::canonical::sha256;
use crate::development::{Layout, payload};
use crate::error::{Result, require};
use crate::evidence::Root;

pub struct Frozen {
    pub image: Vec<u8>,
    /// SHA-256 of the exact `review.json` bytes, which plans bind.
    pub review_sha256: String,
}

fn is_int(value: &Value, expected: usize) -> bool {
    value.as_u64() == Some(expected as u64)
}

pub fn frozen(root: &Root, directory: &Path, layout: &Layout) -> Result<Frozen> {
    let raw = root.private_file(&directory.join("review.json"))?;
    let review: Value = serde_json::from_slice(&raw)?;
    require(
        is_int(&review["schema_version"], 1)
            && review["kind"] == "plaf103_development_image_review",
        "Unsupported frozen review",
    )?;
    let image = root.private_file(&directory.join("image.bin"))?;
    let data = payload(&image, layout)?;
    require(
        review["ota_sha256"] == sha256(&image).as_str()
            && review["payload_sha256"] == sha256(data).as_str()
            && is_int(&review["payload_bytes"], data.len()),
        "Frozen image/review mismatch",
    )?;
    if review["image_kind"] == "stock" {
        require(
            sha256(&image) == crate::ota::STOCK_SHA256 && review["manifest_sha256"].is_null(),
            "Only original stock may omit a build manifest",
        )?;
    } else {
        require(
            review["image_kind"] == "build",
            "Unknown reviewed image kind",
        )?;
        build(root, directory, &review, &image)?;
    }
    Ok(Frozen {
        image,
        review_sha256: sha256(&raw),
    })
}

fn build(root: &Root, directory: &Path, review: &Value, image: &[u8]) -> Result<()> {
    let manifest_raw = root.private_file(&directory.join("build-manifest.json"))?;
    require(
        review["manifest_sha256"] == sha256(&manifest_raw).as_str(),
        "Frozen manifest mismatch",
    )?;
    let m: Value = serde_json::from_slice(&manifest_raw)?;
    let lock: Value = serde_json::from_slice(&std::fs::read(root.join("firmware/sdk.lock.json"))?)?;
    require(
        m["profile"] == "plaf103-firstboot"
            && m["sdk_revision"] == lock["sdk"]["revision"]
            && m["toolchain_sha256"] == lock["toolchain"]["sha256"],
        "Unknown build profile",
    )?;
    let ota: Vec<&Value> = m["artifacts"]
        .as_array()
        .map(|artifacts| {
            artifacts
                .iter()
                .filter(|a| {
                    a["path"]
                        .as_str()
                        .and_then(|path| Path::new(path).file_name())
                        .is_some_and(|name| name == "OTA_All.bin")
                })
                .collect()
        })
        .unwrap_or_default();
    require(
        ota.len() == 1
            && ota[0]["sha256"] == sha256(image).as_str()
            && is_int(&ota[0]["bytes"], image.len()),
        "Manifest/image mismatch",
    )?;
    let details = &review["review"];
    let mains = &details["startup_mains"];
    // Python's `set(...)` of the audit's per-core mapping: its keys.
    let mut names: Vec<&str> = mains
        .as_object()
        .map(|cores| cores.keys().map(String::as_str).collect())
        .unwrap_or_default();
    names.sort();
    let audit = &m["km0_pinmap_audit"];
    require(
        !mains.is_null()
            && *mains == m["firstboot_startup_audit"]["mains"]
            && names == ["hp", "lp"]
            && details["pinmap"] == *audit
            && !audit.is_null()
            && audit["matches_stock"] == true,
        "Frozen linked/startup/pad review missing",
    )?;
    let banks: Vec<&Value> = details["factory_mapping"]
        .as_array()
        .map(|cases| cases.iter().map(|case| &case["bank"]).collect())
        .unwrap_or_default();
    require(
        details["layout"]["sha256"] == sha256(image).as_str()
            && banks.len() == 2
            && is_int(banks[0], 0)
            && is_int(banks[1], 1)
            && [
                "ota_payload_matches",
                "ota_checksum_matches",
                "source_snapshot_unchanged",
                "staged_sources_without_diagnostics",
            ]
            .iter()
            .all(|check| m["checks"][*check] == true),
        "Incomplete build/mapping review",
    )
}
