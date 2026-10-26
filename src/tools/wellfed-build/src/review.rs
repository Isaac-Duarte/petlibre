//! Freeze a reviewed development image offline: `wellfed-build review`.
//!
//! A frozen review is the only form in which `wellfed-flash` accepts an
//! image to install. For a PLAF103 first-boot build it re-derives the
//! build's inputs from the current sources, re-hashes every input, overlay,
//! artifact and archive the manifest names, and re-runs the linked-code
//! audits. It then executes the actual factory loader (`wellfed-emu`) on
//! the pinned baseline with the image alone in each bank. The original stock
//! image is frozen by hash only. No serial port opens, and nothing claims
//! the application boots.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use wellfed_emu::BANKS;
use wellfed_emu::factory::{self, Entry};
use wellfed_formats::canonical::{canonical_of, sha256};
use wellfed_formats::manifest::BuildManifest;
use wellfed_formats::private::{sync_directory, write_new};
use wellfed_formats::review::{
    BuildDetails, Details, FactoryRun, ImageKind, ImageReview, KIND, MappingCase, SCHEMA_VERSION,
    Stop,
};
use wellfed_image::ota::{STOCK_SHA256, container, images};
use wellfed_image::root::Root;

use crate::identity::current_inputs;
use crate::image::audit_build;
use crate::platform::{Linked, amebad};
use crate::{Device, Error, Firmware, Profile, Repo, Result, digest, require, revision};

/// Bank 1's extent; a development image must fit it sector-aligned.
const BANK_BYTES: usize = 0x15_0000;
const SECTOR: usize = 4096;
const ARTIFACTS: [&str; 4] = [
    "OTA_All.bin",
    "km0_image2_all.bin",
    "km0_km4_image2.bin",
    "km4_image2_all.bin",
];
/// The device whose stock image, baseline and factory loader a review pins.
const DEVICE: &str = "plaf103";
const SCOPE: &str =
    "Package, linked code, stock pulls and modeled mapping; application behavior requires review";

/// Review `kind` (for a build, the one in `build`) and freeze it as a new
/// private directory `output`: `image.bin`, `review.json` and, for a build,
/// `build-manifest.json`.
pub fn freeze(repo: &Repo, kind: ImageKind, build: &Path, output: &Path) -> Result<ImageReview> {
    let root = Root::new(&repo.root)?;
    let (output, build) = (repo.root.join(output), repo.root.join(build));
    require(
        !output.exists() && root.is_private(&output),
        "New private output required",
    )?;

    let (image, manifest, review) = match kind {
        ImageKind::Stock => {
            let image = fs::read(Device::new(repo, DEVICE)?.stock(repo)?.image)?;
            require(
                sha256(&image) == STOCK_SHA256,
                "Pinned stock image required",
            )?;
            let details = Details::Stock {
                stock_sha256: STOCK_SHA256.into(),
            };
            (image, None, details)
        }
        ImageKind::Build => {
            require(root.is_private(&build), "Private build required")?;
            let (image, manifest, details) = review(repo, &build)?;
            (image, Some(manifest), Details::Build(Box::new(details)))
        }
    };

    let payload = bank_payload(&image)?;
    let result = ImageReview {
        schema_version: SCHEMA_VERSION,
        kind: KIND.into(),
        image_kind: kind,
        ota_sha256: sha256(&image),
        payload_sha256: sha256(payload),
        payload_bytes: payload.len() as u64,
        manifest_sha256: manifest.as_deref().map(sha256),
        review,
        reviewer_sha256: digest(&std::env::current_exe()?)?,
        application_boot_verified: false,
    };

    root.output_directory(&output)?;
    let mut review_json = canonical_of(&result)?;
    review_json.push(b'\n');
    write_new(&output.join("image.bin"), &image)?;
    write_new(&output.join("review.json"), &review_json)?;
    if let Some(manifest) = &manifest {
        write_new(&output.join("build-manifest.json"), manifest)?;
    }
    sync_directory(&output)?;
    if let Some(parent) = output.parent() {
        sync_directory(parent)?;
    }
    Ok(result)
}

/// The application payload, sector aligned within the bank footprint, with
/// both cores at their exact addresses.
pub fn bank_payload(image: &[u8]) -> Result<&[u8]> {
    let payload = container(image)?;
    require(
        !payload.is_empty() && payload.len() <= BANK_BYTES && payload.len() % SECTOR == 0,
        "Development image exceeds aligned bank-1 footprint",
    )?;
    images(payload)?;
    Ok(payload)
}

/// Everything a build review checks, returning the image, the exact manifest
/// bytes and the review details.
fn review(repo: &Repo, build: &Path) -> Result<(Vec<u8>, Vec<u8>, BuildDetails)> {
    let raw = fs::read(build.join("manifest.json"))?;
    let m: BuildManifest = serde_json::from_slice(&raw)?;
    let lock = amebad::lock(repo)?;
    require(
        m.device == DEVICE && m.profile == Profile::Firstboot.name(),
        "Only the reviewed PLAF103 UART startup profile is supported",
    )?;
    let device = Device::new(repo, &m.device)?;
    let Some(firmware) = m.firmware.as_deref() else {
        return Err(Error::Refused(
            "Build manifest names no firmware; rebuild it".into(),
        ));
    };
    let firmware = Firmware::new(repo, firmware)?;
    require(
        m.sdk_revision == lock.sdk.revision && m.toolchain_sha256 == lock.toolchain.sha256,
        "Unexpected SDK/toolchain",
    )?;
    let sdk = build.join("sdk");
    require(
        revision(&sdk)? == m.sdk_revision,
        "SDK worktree revision differs",
    )?;

    // Inputs, overlays, artifacts and archives, each re-hashed.
    let inputs = current_inputs(repo, &device, Profile::Firstboot, &firmware)?;
    require(
        m.inputs == inputs,
        "Build inputs are stale or incomplete; rebuild before review",
    )?;
    for (name, expected) in &m.inputs {
        require(
            digest(&local(&repo.root, name)?)? == *expected,
            "Build input differs",
        )?;
    }
    for (name, expected) in &m.sdk_overrides {
        require(
            digest(&local(&sdk, name)?)? == *expected,
            "SDK overlay differs",
        )?;
    }
    let files = artifacts(build, &m)?;
    let file = |name: &str| files.get(name).map_or(&[][..], Vec::as_slice);
    let image = file("OTA_All.bin").to_vec();
    let payload = bank_payload(&image)?;
    require(
        payload == file("km0_km4_image2.bin")
            && payload == [file("km0_image2_all.bin"), file("km4_image2_all.bin")].concat(),
        "Both core packages must exactly compose the OTA payload",
    )?;
    let mut cores: Vec<&str> = Vec::new();
    for archive in &m.compiled_archives {
        require(
            !cores.contains(&archive.core.as_str()),
            "Duplicate core archive",
        )?;
        let path = local(build, &archive.path)?;
        require(digest(&path)? == archive.sha256, "Compiled archive differs")?;
        cores.push(&archive.core);
    }
    cores.sort_unstable();
    require(cores == ["KM0", "KM4"], "Both compiled cores required")?;

    // The linked-code audits, rerun, must be the ones the build recorded.
    let Linked::Amebad(fresh) = audit_build(repo, build)?;
    let recorded = &m.audits;
    require(
        recorded.firstboot_startup_audit.as_ref() == Some(&fresh.firstboot_startup_audit)
            && recorded.ram_park_audit.as_ref() == Some(&fresh.ram_park_audit)
            && recorded.sdk_hook_audit.as_ref() == Some(&fresh.sdk_hook_audit),
        "Linked-code audits differ from build manifest",
    )?;
    require(
        recorded.km0_pinmap_audit.as_ref() == Some(&fresh.km0_pinmap_audit),
        "Linked pad table differs",
    )?;

    let (baseline, rom) = factory::inputs(&repo.root)?;
    let factory_mapping = mapping_cases(&baseline, &rom, &image)?;

    // Refuse edits made during the (long) audits and emulation.
    require(
        fs::read(build.join("manifest.json"))? == raw
            && current_inputs(repo, &device, Profile::Firstboot, &firmware)? == inputs,
        "Build changed during review",
    )?;
    for artifact in &m.artifacts {
        require(
            digest(&local(build, &artifact.path)?)? == artifact.sha256,
            "Artifact changed during review",
        )?;
    }

    let details = BuildDetails {
        startup_mains: fresh.firstboot_startup_audit.mains,
        layout: fresh.layout_audit,
        pinmap: fresh.km0_pinmap_audit,
        factory_mapping,
        scope: SCOPE.into(),
    };
    Ok((image, raw, details))
}

/// The four packaged artifacts by file name, each matching its manifest entry.
fn artifacts(build: &Path, m: &BuildManifest) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for artifact in &m.artifacts {
        let path = local(build, &artifact.path)?;
        let data = fs::read(&path)?;
        require(
            data.len() as u64 == artifact.bytes && sha256(&data) == artifact.sha256,
            "Build artifact differs",
        )?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        files.insert(name.unwrap_or_default(), data);
    }
    require(
        files.len() == m.artifacts.len() && files.keys().eq(ARTIFACTS),
        "Incomplete packaged artifacts",
    )?;
    Ok(files)
}

/// A regular file `name` inside `parent`.
fn local(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name).canonicalize()?;
    require(
        path.starts_with(parent.canonicalize()?) && path.is_file(),
        "Manifest file escapes its root",
    )?;
    Ok(path)
}

/// The actual factory loader on `current` with `image` as the only valid
/// bank, for each bank: its bank check maps exactly the packaged cores, and
/// the ordinary selection boots it. Nothing here is ever sent to a device.
pub fn mapping_cases(current: &[u8], rom: &[u8], image: &[u8]) -> Result<Vec<MappingCase>> {
    let payload = container(image)?;
    let split = images(payload)?
        .iter()
        .find(|region| region.name == "km4_xip")
        .map(|region| region.payload_header_offset)
        .ok_or_else(|| Error::Refused("Missing km4_xip image".into()))?;

    let mut cases = Vec::new();
    for (bank, start) in (0u8..).zip(BANKS) {
        let start = start as usize;
        let mut fixture = current.to_vec();
        fixture[start..start + payload.len()].copy_from_slice(payload);
        // This fixture deliberately selects just this bank.
        let other = BANKS[usize::from(1 - bank)] as usize;
        fixture[other..other + 8].fill(0);

        let checked = factory::execute(&fixture, rom, Entry::Check(bank))?;
        require(
            checked.stop == Stop::Returned && checked.result == 1,
            "Actual factory instructions reject diagnostic headers",
        )?;
        require(
            checked.configs.len() == 4 && checked.commands == [[0, 1], [1, 1]],
            "Unexpected factory mapper sequence",
        )?;
        let cores = [
            (0x0C00_0000u32, start, split),
            (0x0E00_0000, start + split, payload.len() - split),
        ];
        for (index, (virtual_start, physical, length)) in cores.into_iter().enumerate() {
            require(
                maps(&checked, index, virtual_start, physical, length),
                "Factory mapping does not cover packaged diagnostic",
            )?;
        }

        let chosen = factory::execute(&fixture, rom, Entry::Select)?;
        require(
            chosen.stop == Stop::Selected && chosen.result == u32::from(bank) + 1,
            "Single-valid diagnostic bank selection differs",
        )?;
        cases.push(MappingCase {
            bank,
            bank_check: checked,
            selection: chosen,
        });
    }
    Ok(cases)
}

/// Whether core `index`'s final mapping covers `length` bytes at flash
/// offset `physical` from `virtual_start`, subtractively.
fn maps(
    run: &FactoryRun,
    index: usize,
    virtual_start: u32,
    physical: usize,
    length: usize,
) -> bool {
    let Some(last) = run.configs.get(index * 2 + 1) else {
        return false;
    };
    let virtual_start = i64::from(virtual_start);
    let (Ok(physical), Ok(length)) = (i64::try_from(physical), i64::try_from(length)) else {
        return false;
    };
    i64::from(last.start) == virtual_start
        && i64::from(last.end) == virtual_start + length - 1
        && last.minus == 1
        && i64::from(last.offset) == virtual_start - 0x0800_0000 - physical
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_must_be_a_valid_container_that_fits_the_aligned_bank() {
        assert!(bank_payload(b"OTA").is_err());
        assert!(bank_payload(&[0; 64]).is_err());
    }

    #[test]
    fn an_unknown_profile_is_refused_before_anything_is_written() {
        let repo = Repo::here().unwrap();
        let private = repo.root.join("private");
        fs::create_dir_all(&private).unwrap();
        let scratch = tempfile::tempdir_in(&private).unwrap();
        let build = scratch.path().join("build");
        fs::create_dir(&build).unwrap();
        fs::write(build.join("manifest.json"), r#"{"profile": "bringup"}"#).unwrap();

        let output = scratch.path().join("output");
        let error = freeze(&repo, ImageKind::Build, &build, &output).unwrap_err();
        assert!(error.to_string().contains("startup profile"), "{error}");
        assert!(!output.exists());
    }

    #[test]
    fn output_outside_private_is_refused() {
        let repo = Repo::here().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let output = outside.path().join("review");
        assert!(freeze(&repo, ImageKind::Stock, Path::new("unused"), &output).is_err());
        assert!(!output.exists());
    }
}
