//! The source-bound startup identity: a hash of the device, the profile,
//! the firmware and its variant, the update key and every input file's
//! digest.
//!
//! Every Rust-linked image prints it (`PLFW1 start build=…`), the manifest
//! records it, and the bench reuses a frozen review whose identity matches
//! what a build of the current sources would embed. The builder's own
//! sources are inputs, so a changed recipe or audit is a new identity.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use wellfed_formats::credentials::OtaKeyFile;
use wellfed_formats::hex::is_lower_hex;
use wellfed_formats::manifest::IdentitySnapshot;

use crate::{
    Device, Error, Firmware, Profile, Repo, Result, Stage, Variant, digest, relative, require,
};

/// Crates linked into, or testing, every firmware build: the whole SDK. The
/// selected firmware's directory joins them; other firmwares and host tools
/// such as wellfed-local and wellfed-flash never change a build's identity.
const SDK_CRATES: &str = "src/sdk";

/// The builder's own sources (its fixture tests are not), plus the
/// wellfed-flash modules whose OTA container and region checks the layout
/// and park audits share.
const BUILDER_SOURCES: &str = "src/tools/wellfed-build/src";
const BUILDER_SHARED: [&str; 3] = [
    "src/tools/wellfed-build/Cargo.toml",
    "src/tools/wellfed-flash/src/ota.rs",
    "src/tools/wellfed-flash/src/error.rs",
];

/// FW10 update-signing key from `wellfed-local ota-keygen`. Only its public
/// half reaches the build, the identity and the manifest.
const OTA_KEY: &str = "private/ota-key.json";

/// Tracked plus untracked, non-ignored files below the root-relative
/// directories `dirs`.
///
/// Ignored files (editor backups, caches, build output) do not change the
/// identity; `target` is excluded only as a directory below the checkout, so
/// the checkout's own location never matters.
fn rust_sources(repo: &Repo, dirs: &[&str]) -> Result<Vec<PathBuf>> {
    let listing = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .args(dirs)
        .current_dir(&repo.root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .ok_or_else(|| {
            Error::Refused("Source identity requires a Git checkout of this repository".into())
        })?
        .stdout;

    let mut paths: Vec<PathBuf> = String::from_utf8_lossy(&listing)
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(|name| repo.root.join(name))
        // Tracked files deleted from the working tree are simply not inputs.
        .filter(|path| {
            path.strip_prefix(&repo.root)
                .is_ok_and(|inside| !inside.iter().any(|part| part == "target"))
                && path.is_file()
        })
        .collect();
    paths.sort();
    paths.dedup();

    require(
        !paths.is_empty(),
        "No Rust firmware sources found for the source identity",
    )?;
    Ok(paths)
}

/// Every file a profile's image depends on, in a stable order.
fn source_inputs(
    repo: &Repo,
    device: &Device,
    profile: Profile,
    firmware: &Firmware,
) -> Result<Vec<PathBuf>> {
    let mut paths = device.platform(repo).inputs(repo, profile);
    paths.push(repo.path(&device.config_path()));
    paths.extend(rust_sources(repo, &[BUILDER_SOURCES])?);
    paths.extend(BUILDER_SHARED.map(|p| repo.path(p)));

    if profile.has(Stage::Rust) {
        paths.extend(["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"].map(|p| repo.path(p)));
        paths.extend(rust_sources(repo, &[SDK_CRATES, &firmware.dir()])?);
    }
    if profile.has(Stage::Stock) {
        paths.push(device.stock(repo)?.layout);
    }

    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    Ok(paths)
}

/// Each input's digest, keyed by its root-relative path as the manifest
/// records it.
fn digests(repo: &Repo, paths: &[PathBuf]) -> Result<BTreeMap<String, String>> {
    paths
        .iter()
        .map(|path| Ok((relative(path, &repo.root)?, digest(path)?)))
        .collect()
}

/// The current inputs of a profile with their digests.
pub fn current_inputs(
    repo: &Repo,
    device: &Device,
    profile: Profile,
    firmware: &Firmware,
) -> Result<BTreeMap<String, String>> {
    digests(repo, &source_inputs(repo, device, profile, firmware)?)
}

/// The update-signing public key built into recovery-role images, or None
/// without a key file.
pub(crate) fn ota_public_key(repo: &Repo, profile: Profile) -> Result<Option<String>> {
    let path = repo.path(OTA_KEY);
    if !profile.has(Stage::Firstboot) || !path.is_file() {
        return Ok(None);
    }

    let key: OtaKeyFile = wellfed_formats::load(&path)?;
    require(
        is_lower_hex(&key.public_key, 64),
        "Update key file needs a 64-character lowercase hex public key",
    )?;
    Ok(Some(key.public_key))
}

/// What one build binds into its identity, besides its inputs.
pub(crate) struct Subject<'a> {
    pub(crate) device: &'a Device,
    pub(crate) profile: Profile,
    pub(crate) firmware: &'a Firmware,
    pub(crate) variant: Option<&'a Variant>,
}

/// Bind the identity to the device, the profile, the firmware a Rust
/// profile links, the variant, the update key and root-relative input
/// digests: SHA-256 of their canonical (sorted, compact) JSON.
pub(crate) fn source_identity(
    subject: &Subject,
    inputs: &BTreeMap<String, String>,
    ota_key: Option<&str>,
) -> Result<String> {
    let snapshot = IdentitySnapshot {
        device: subject.device.name(),
        profile: subject.profile.name(),
        inputs,
        firmware: subject
            .profile
            .has(Stage::Rust)
            .then(|| subject.firmware.name()),
        variant: subject.variant.map(Variant::name),
        ota_public_key: ota_key,
    };
    Ok(snapshot.identity()?)
}

/// Which first-boot identity `source-id` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceId {
    /// The bare identity of the sources, which check-cross compiles against.
    Bare,
    /// The one a build of this variant would embed (update key included),
    /// which the bench matches reviews by.
    Image,
}

/// The first-boot identity of `firmware` on `device`. A bare identity binds
/// no variant.
pub fn source_id(
    repo: &Repo,
    device: &Device,
    firmware: &Firmware,
    variant: Option<&Variant>,
    which: SourceId,
) -> Result<String> {
    let profile = Profile::Firstboot;
    let inputs = current_inputs(repo, device, profile, firmware)?;
    let mut subject = Subject {
        device,
        profile,
        firmware,
        variant,
    };
    match which {
        SourceId::Image => {
            let key = ota_public_key(repo, profile)?;
            source_identity(&subject, &inputs, key.as_deref())
        }
        SourceId::Bare => {
            subject.variant = None;
            source_identity(&subject, &inputs, None)
        }
    }
}
