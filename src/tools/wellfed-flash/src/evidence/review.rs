//! Frozen image reviews made by `wellfed-build review`.
//!
//! The review re-runs the build's linked-code audits under the pinned
//! toolchain and the factory-loader emulation; this side only checks that a
//! frozen review is complete and still matches its image. Reviews frozen by
//! the former Python reviewer have the same shape and still read.

use std::path::Path;

use crate::error::{Result, refuse, require};
use crate::evidence::Root;
use crate::writers::development::{Layout, payload};
use wellfed_formats::canonical::sha256;
use wellfed_formats::lock::SdkLock;
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::review::{BuildDetails, Details, ImageKind, ImageReview, KIND};

pub struct Frozen {
    pub image: Vec<u8>,
    /// SHA-256 of the exact `review.json` bytes, which plans bind.
    pub review_sha256: String,
    pub review: ImageReview,
    /// The verified build manifest; `None` only for the original stock image.
    pub manifest: Option<BuildManifest>,
}

pub fn frozen(root: &Root, directory: &Path, layout: &Layout) -> Result<Frozen> {
    let raw = root.private_file(&directory.join("review.json"))?;
    let review: ImageReview = serde_json::from_slice(&raw)?;
    require(
        review.schema_version == 1 && review.kind == KIND,
        "Unsupported frozen review",
    )?;

    let image = root.private_file(&directory.join("image.bin"))?;
    let data = payload(&image, layout)?;
    require(
        review.ota_sha256 == sha256(&image)
            && review.payload_sha256 == sha256(data)
            && u64::try_from(data.len()).is_ok_and(|bytes| bytes == review.payload_bytes),
        "Frozen image/review mismatch",
    )?;

    let manifest = match (review.image_kind, &review.review) {
        (ImageKind::Stock, Details::Stock { .. }) => {
            require(
                sha256(&image) == crate::ota::STOCK_SHA256 && review.manifest_sha256.is_none(),
                "Only original stock may omit a build manifest",
            )?;
            None
        }
        (ImageKind::Build, Details::Build(details)) => {
            Some(build(root, directory, &review, details, &image)?)
        }
        _ => return refuse("Review details do not match the reviewed image kind"),
    };

    Ok(Frozen {
        image,
        review_sha256: sha256(&raw),
        review,
        manifest,
    })
}

fn build(
    root: &Root,
    directory: &Path,
    review: &ImageReview,
    details: &BuildDetails,
    image: &[u8],
) -> Result<BuildManifest> {
    let manifest_raw = root.private_file(&directory.join("build-manifest.json"))?;
    require(
        review.manifest_sha256.as_deref() == Some(&sha256(&manifest_raw)),
        "Frozen manifest mismatch",
    )?;

    let m: BuildManifest = serde_json::from_slice(&manifest_raw)?;
    let lock: SdkLock = wellfed_formats::load(&root.join("src/firmware/vendor/sdk.lock.json"))?;
    // Older builds recorded no device and named the profile
    // `plaf103-firstboot`. Bank 0's recovery image is one, and a give-way or
    // a reinstall names its review, so those still read.
    let firstboot = (m.device == "plaf103" && m.profile == "firstboot")
        || (m.device.is_empty() && m.profile == "plaf103-firstboot");
    require(
        firstboot
            && m.sdk_revision == lock.sdk.revision
            && m.toolchain_sha256 == lock.toolchain.sha256,
        "Unknown build profile",
    )?;

    let ota = m.artifact("OTA_All.bin");
    require(
        ota.is_some_and(|ota| ota.sha256 == sha256(image) && ota.bytes == image.len() as u64),
        "Manifest/image mismatch",
    )?;

    let mains = &details.startup_mains;
    // The audit's per-core mapping: exactly both cores.
    let names: Vec<&str> = mains.keys().map(String::as_str).collect();
    let startup = m.audits.firstboot_startup_audit.as_ref();
    let audit = m.audits.km0_pinmap_audit.as_ref();
    require(
        startup.map(|s| &s.mains) == Some(mains)
            && names == ["hp", "lp"]
            && audit.is_some_and(|audit| details.pinmap == *audit && audit.matches_stock),
        "Frozen linked/startup/pad review missing",
    )?;

    let banks: Vec<u8> = details.factory_mapping.iter().map(|c| c.bank).collect();
    require(
        details.layout.sha256 == sha256(image)
            && banks == [0, 1]
            && m.checks.ota_payload_matches
            && m.checks.ota_checksum_matches
            && m.checks.source_snapshot_unchanged
            && m.checks.staged_sources_without_diagnostics,
        "Incomplete build/mapping review",
    )?;
    Ok(m)
}
