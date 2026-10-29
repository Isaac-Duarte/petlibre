//! The chip families a device builds on.
//!
//! A platform is code, not configuration: its SDK overlays anchor on exact
//! text in a pinned SDK and its audits read disassembly, and both must fail
//! closed. A device names its platform in `device.json`; adding a chip means
//! adding a variant here and a module beside [`amebad`].
//!
//! The platform-neutral flow (identity, manifest, policies) is in
//! [`crate::image`]; a platform compiles, packages and audits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use wellfed_formats::device::PlatformConfig;
use wellfed_formats::manifest::{Artifact, Audits, CompiledArchive};

use crate::environment::Rust;
use crate::{Device, Firmware, Profile, Provisioning, Repo, Result, Variant};

pub mod amebad;

/// Every platform's firmware compilation targets, whose cargo overrides a
/// build refuses.
pub(crate) const TARGETS: [&str; 2] = amebad::TARGETS;

#[derive(Debug, Clone)]
pub(crate) enum Platform {
    Amebad(amebad::Amebad),
}

/// One profile's build, as the platform-neutral flow hands it over.
pub(crate) struct Job<'a> {
    pub(crate) repo: &'a Repo,
    pub(crate) device: &'a Device,
    pub(crate) profile: Profile,
    pub(crate) firmware: &'a Firmware,
    pub(crate) variant: Option<&'a Variant>,
    pub(crate) provisioning: Provisioning,
    /// The toolchain [`Platform::bootstrap`] verified.
    pub(crate) toolchain: &'a Path,
    /// The profile's output directory.
    pub(crate) output: PathBuf,
    /// The build's private log.
    pub(crate) log: PathBuf,
    /// The compiler of a Rust profile.
    pub(crate) rust: Option<&'a Rust>,
    pub(crate) identity: &'a str,
    pub(crate) ota_key: Option<&'a str>,
    /// The input digests the identity was computed from.
    pub(crate) inputs: &'a BTreeMap<String, String>,
    /// The platform-neutral checks that nothing the build read changed,
    /// run after compilation and before the audits.
    pub(crate) recheck: &'a dyn Fn() -> Result<()>,
}

/// What a platform build adds to the manifest.
pub(crate) struct Built {
    pub(crate) sdk_revision: String,
    pub(crate) toolchain_sha256: String,
    pub(crate) toolchain_files: BTreeMap<String, String>,
    pub(crate) sdk_overrides: BTreeMap<String, String>,
    pub(crate) artifacts: Vec<Artifact>,
    pub(crate) compiled_archives: Vec<CompiledArchive>,
    pub(crate) banner: bool,
    pub(crate) required_symbols: Vec<String>,
    pub(crate) audits: Audits,
}

/// A finished first-boot build's audits, keyed as its manifest records them.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Linked {
    Amebad(amebad::audit::Linked),
}

impl Platform {
    pub(crate) fn new(repo: &Repo, config: &PlatformConfig) -> Self {
        match config {
            PlatformConfig::Amebad {
                entry,
                firstboot_mains,
                bluetooth,
            } => Self::Amebad(amebad::Amebad::new(
                repo,
                entry,
                firstboot_mains,
                bluetooth.as_deref(),
            )),
        }
    }

    /// Fetch (once) and verify the pinned SDK and toolchain; returns the
    /// toolchain directory.
    pub(crate) fn bootstrap(&self, repo: &Repo) -> Result<PathBuf> {
        match self {
            Self::Amebad(_) => amebad::toolchain::bootstrap(repo),
        }
    }

    /// The platform's and the board's files a profile's image of
    /// `firmware` depends on; refused when the firmware needs a board file
    /// the device lacks.
    pub(crate) fn inputs(
        &self,
        repo: &Repo,
        profile: Profile,
        provisioning: Provisioning,
        firmware: &Firmware,
    ) -> Result<Vec<PathBuf>> {
        match self {
            Self::Amebad(amebad) => amebad.inputs(repo, profile, provisioning, firmware),
        }
    }

    pub(crate) fn build(&self, job: &Job) -> Result<Built> {
        match self {
            Self::Amebad(amebad) => amebad.build(job),
        }
    }

    /// Re-run a finished first-boot build's linked-image audits.
    pub(crate) fn audit(
        &self,
        repo: &Repo,
        device: &Device,
        build: &Path,
        archives: &[(String, PathBuf)],
        firmware: &Firmware,
    ) -> Result<Linked> {
        match self {
            Self::Amebad(_) => Ok(Linked::Amebad(amebad::audit::linked(
                repo,
                &device.stock(repo)?,
                build,
                &amebad::archives(archives)?,
                firmware,
            )?)),
        }
    }
}
