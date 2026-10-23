//! The source-bound startup identity: a hash of the profile, the variant,
//! the update key and every input file's digest.
//!
//! Every Rust-linked image prints it (`PLFW1 start build=…`), the manifest
//! records it, and the bench reuses a frozen review whose identity matches
//! what a build of the current sources would embed. The builder's own
//! sources are inputs, so a changed recipe or audit is a new identity.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use petlibre_formats::credentials::OtaKeyFile;
use petlibre_formats::hex::is_lower_hex;
use petlibre_formats::manifest::IdentitySnapshot;

use crate::{Core, Error, Profile, Repo, Result, Variant, digest, relative, require};

/// Crates linked into, or testing, the firmware. Host tools such as
/// petlibre-local and petlibre-flash never change a firmware build's identity.
const FIRMWARE_CRATES: [&str; 8] = [
    "petlibre-core",
    "petlibre-ffi",
    "petlibre-testkit",
    "petlibre-amebad",
    "petlibre-amebad-common",
    "petlibre-amebad-sdk",
    "petlibre-firstboot",
    "petlibre-sim",
];

/// The builder's own sources (its fixture tests are not), plus the
/// petlibre-flash modules whose OTA container and region checks the layout
/// and park audits share.
const BUILDER_SOURCES: &str = "petlibre-build/src";
const BUILDER_SHARED: [&str; 3] = [
    "crates/petlibre-build/Cargo.toml",
    "crates/petlibre-flash/src/ota.rs",
    "crates/petlibre-flash/src/error.rs",
];

/// The application's generated-header inputs: read from ignored private
/// files; only their hashes reach the manifest.
pub(crate) const WIFI_CREDENTIALS: &str = "private/hotspot.json";
pub(crate) const TRANSPORT_CREDENTIALS: &str = "private/fw-mqtt.json";
/// FW10 update-signing key from `petlibre-local ota-keygen`. Only its public
/// half reaches the build, the identity and the manifest.
const OTA_KEY: &str = "private/ota-key.json";
/// The SDK `example_entry.c` replacement; it includes both credential headers.
pub(crate) const ENTRY: &str = "firmware/ports/amebad/example_entry.c";
pub(crate) const TOOL_LOCK: &str = "firmware/probe/toolchain.lock.json";

/// The UART-only SDK mains of the first-boot profile.
pub(crate) fn firstboot_sources(repo: &Repo) -> [(Core, PathBuf); 2] {
    [
        (Core::Lp, repo.path("firmware/ports/amebad/firstboot_km0.c")),
        (Core::Hp, repo.path("firmware/ports/amebad/firstboot_km4.c")),
    ]
}

/// Tracked plus untracked, non-ignored files of `crates` (names below
/// `crates/`).
///
/// Ignored files (editor backups, caches, build output) do not change the
/// identity; `target` is excluded only as a directory below crates/, so the
/// checkout's own location never matters.
fn rust_sources(repo: &Repo, crates: &[&str]) -> Result<Vec<PathBuf>> {
    let rust = repo.path("crates");
    let listing = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .args(crates.iter().map(|name| format!("crates/{name}")))
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
            path.strip_prefix(&rust)
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
fn source_inputs(repo: &Repo, profile: Profile) -> Result<Vec<PathBuf>> {
    let mut paths = vec![repo.path("firmware/sdk.lock.json"), repo.path(TOOL_LOCK)];
    paths.extend(rust_sources(repo, &[BUILDER_SOURCES])?);
    paths.extend(BUILDER_SHARED.map(|p| repo.path(p)));

    if profile.rust() {
        paths.extend(
            [ENTRY, "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"].map(|p| repo.path(p)),
        );
        // ENTRY includes both generated credential headers.
        paths.extend([WIFI_CREDENTIALS, TRANSPORT_CREDENTIALS].map(|p| repo.path(p)));
        paths.extend(rust_sources(repo, &FIRMWARE_CRATES)?);
    }
    if profile.model() {
        paths.push(repo.layout());
    }
    if profile.firstboot() {
        paths.extend(firstboot_sources(repo).map(|(_, path)| path));
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
pub fn current_inputs(repo: &Repo, profile: Profile) -> Result<BTreeMap<String, String>> {
    digests(repo, &source_inputs(repo, profile)?)
}

/// The update-signing public key built into recovery-role images, or None
/// without a key file.
pub(crate) fn ota_public_key(repo: &Repo, profile: Profile) -> Result<Option<String>> {
    let path = repo.path(OTA_KEY);
    if !profile.firstboot() || !path.is_file() {
        return Ok(None);
    }

    let key: OtaKeyFile = petlibre_formats::load(&path)?;
    require(
        is_lower_hex(&key.public_key, 64),
        "Update key file needs a 64-character lowercase hex public key",
    )?;
    Ok(Some(key.public_key))
}

/// Bind the identity to a profile, variant, update key and root-relative
/// input digests: SHA-256 of their canonical (sorted, compact) JSON.
pub(crate) fn source_identity(
    profile: Profile,
    inputs: &BTreeMap<String, String>,
    variant: Option<Variant>,
    ota_key: Option<&str>,
) -> Result<String> {
    let snapshot = IdentitySnapshot {
        profile: profile.name(),
        inputs,
        variant: variant.map(Variant::name),
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
    Image(Option<Variant>),
}

pub fn source_id(repo: &Repo, which: SourceId) -> Result<String> {
    let profile = Profile::Plaf103Firstboot;
    let inputs = current_inputs(repo, profile)?;
    match which {
        SourceId::Image(variant) => {
            let key = ota_public_key(repo, profile)?;
            source_identity(profile, &inputs, variant, key.as_deref())
        }
        SourceId::Bare => source_identity(profile, &inputs, None, None),
    }
}
