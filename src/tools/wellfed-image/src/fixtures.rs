//! Synthetic images and frozen reviews for host tests, here and in the
//! crates that verify reviews through [`crate::review::frozen`]
//! (the `fixtures` feature). Nothing here is a boot candidate: the images
//! are structurally valid containers with patterned bodies.

use std::fs;
use std::io;
use std::path::Path;

use wellfed_formats::canonical::sha256;
use serde_json::{Value, json};

use crate::ota::IMAGE_MAGIC;

/// A structurally valid synthetic payload: never a boot candidate.
pub fn synthetic_payload(km0: usize, km4: usize, seed: u8) -> Vec<u8> {
    fn image(out: &mut Vec<u8>, address: u32, body: usize, seed: u8) {
        let size = u32::try_from(body).unwrap_or(u32::MAX);
        for word in [IMAGE_MAGIC[0], IMAGE_MAGIC[1], size, address] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&[0xff; 16]);
        out.extend((0..body).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
    }

    let mut out = Vec::new();
    image(&mut out, 0x0C00_0020, km0, seed);
    image(&mut out, 0x0008_3000, 64, seed);
    out.resize((out.len() + 4095) & !4095, 0xff);
    image(&mut out, 0x0E00_0020, km4, seed ^ 0x55);
    image(&mut out, 0x1000_5000, 64, seed);
    image(&mut out, 0x0200_0020, 0, seed);
    out.resize((out.len() + 4095) & !4095, 0xff);
    out
}

/// `payload` in the OTA container the factory loader's profile expects.
pub fn wrap(payload: &[u8]) -> Vec<u8> {
    let sum = payload
        .iter()
        .fold(0u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
    let mut out = Vec::new();
    for word in [
        0xffff_ffff,
        1,
        u32::from_le_bytes(*b"OTA1"),
        24,
        sum,
        u32::try_from(payload.len()).unwrap_or(u32::MAX),
        32,
        0xffff_ffff,
    ] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// A synthetic image whose KM0 body carries `text`, as a linked image
/// carries its startup identity.
pub fn image_carrying(text: &[u8]) -> Vec<u8> {
    let mut payload = synthetic_payload(9000, 30000, 3);
    payload[32..32 + text.len()].copy_from_slice(text);
    wrap(&payload)
}

fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, data)
}

/// Freeze `image` in `directory` as a first-boot build review that
/// [`crate::review::frozen`] accepts, writing the SDK lock it
/// checks into `checkout`. `fields` (an object) is merged into the build
/// manifest. Returns the review and the manifest as written.
pub fn freeze_build(
    checkout: &Path,
    directory: &Path,
    image: &[u8],
    fields: &Value,
) -> io::Result<(Value, Value)> {
    let lock = json!({"sdk": {"url": "sdk", "revision": "r1"},
                      "toolchain": {"url": "toolchain", "sha256": "t".repeat(64), "directory": "asdk"}});
    write(
        &checkout.join("src/firmware/vendor/sdk.lock.json"),
        lock.to_string().as_bytes(),
    )?;

    let mains = json!({"hp": {"address": "0x1"}, "lp": {"address": "0x2"}});
    let pinmap = json!({"matches_stock": true, "runtime_address": "0x3"});
    let mut manifest = json!({
        "device": "plaf103", "profile": "firstboot", "sdk_revision": "r1", "toolchain_sha256": "t".repeat(64),
        "artifacts": [{"path": "image/OTA_All.bin", "sha256": sha256(image), "bytes": image.len()}],
        "firstboot_startup_audit": {"mains": mains}, "km0_pinmap_audit": pinmap,
        "checks": {"ota_payload_matches": true, "ota_checksum_matches": true,
                   "source_snapshot_unchanged": true, "staged_sources_without_diagnostics": true},
    });
    if let (Some(manifest), Some(fields)) = (manifest.as_object_mut(), fields.as_object()) {
        manifest.extend(fields.clone());
    }

    let payload = image.get(32..).unwrap_or_default();
    // The loader runs themselves are not checked, only that both banks ran.
    let run = json!({"stop": "0x8f000", "result": 1, "configs": [], "commands": [],
                     "final_registers": [[0, 0, 0, 0], [0, 0, 0, 0]]});
    let case = |bank: u8| json!({"bank": bank, "bank_check": run, "selection": run});
    let review = json!({
        "schema_version": 1, "kind": "plaf103_development_image_review", "image_kind": "build",
        "ota_sha256": sha256(image), "payload_sha256": sha256(payload), "payload_bytes": payload.len(),
        "manifest_sha256": sha256(manifest.to_string().as_bytes()),
        "review": {"startup_mains": mains, "pinmap": pinmap, "layout": {"sha256": sha256(image)},
                   "factory_mapping": [case(0), case(1)], "scope": "unit"},
        "reviewer_sha256": "0".repeat(64), "application_boot_verified": false,
    });

    write(&directory.join("image.bin"), image)?;
    write(
        &directory.join("build-manifest.json"),
        manifest.to_string().as_bytes(),
    )?;
    write(
        &directory.join("review.json"),
        review.to_string().as_bytes(),
    )?;
    Ok((review, manifest))
}
