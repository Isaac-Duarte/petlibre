//! Local firmware builds, without connecting to the feeder.
//!
//! A build combines three things:
//! - a **device** (`src/firmware/devices/<name>/device.json`, [`device`]): the board,
//!   naming its platform and the board-level files that platform stages;
//! - a **firmware** (`src/firmware/wellfed/<name>`, [`firmware`]): the KM4
//!   application crate, whose `[package.metadata.wellfed]` table declares the
//!   devices, policies and image variants it has;
//! - a **profile** ([`Profile`]): which stages of the ladder from the
//!   unchanged SDK example to the reviewed first-boot image run.
//!
//! The platform ([`platform`]) is code: its SDK and toolchain are pinned and
//! verified on every run, each profile builds in its own private SDK worktree
//! overlaid from the pinned revision, and the linked images are audited and
//! described in `manifest.json`, which the frozen review re-checks. The
//! startup identity every image prints is a hash of its exact inputs
//! ([`identity`]), so a review of the same sources can be reused instead of
//! rebuilt.

#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

pub mod catalog;
pub mod device;
pub(crate) mod environment;
mod error;
pub mod firmware;
pub mod identity;
pub mod image;
pub mod platform;
pub mod review;
pub mod signing;

pub use device::Device;
pub use error::{Error, Result, require};
pub use firmware::{Firmware, Variant};
pub(crate) use wellfed_formats::canonical::sha256;

/// The build profiles, from the unchanged SDK example to the reviewed
/// UART-only first-boot image. Each adds stages to the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    /// The unchanged SDK example.
    Baseline,
    /// The Rust application on the SDK's own layout.
    Bringup,
    /// The Rust application on the device's stock-derived layout.
    Device,
    /// The device image with the UART-only first-boot mains: what the bench
    /// reviews and installs.
    Firstboot,
}

/// One step of the profile ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Links the Rust application and the SDK runtime.
    Rust,
    /// Uses the device's stock-derived OTA bank addresses and pad table.
    Stock,
    /// Stages the UART-only first-boot mains; the image joins Wi-Fi, the
    /// transport and updates, and carries the firmware's policies.
    Firstboot,
}

impl Profile {
    /// The manifest and output-directory name; also what the identity binds.
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Bringup => "bringup",
            Self::Device => "device",
            Self::Firstboot => "firstboot",
        }
    }

    pub fn stages(self) -> &'static [Stage] {
        match self {
            Self::Baseline => &[],
            Self::Bringup => &[Stage::Rust],
            Self::Device => &[Stage::Rust, Stage::Stock],
            Self::Firstboot => &[Stage::Rust, Stage::Stock, Stage::Firstboot],
        }
    }

    pub fn has(self, stage: Stage) -> bool {
        self.stages().contains(&stage)
    }
}

/// `name` as a device or firmware directory name: lowercase letters,
/// digits and dashes, so it never leaves its parent directory.
pub(crate) fn checked_name<'a>(kind: &str, name: &'a str) -> Result<&'a str> {
    require(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        format!("{kind} names are lowercase letters, digits and dashes: {name:?}"),
    )?;
    Ok(name)
}

/// The checkout the tool was built from, and the paths it reads and writes.
#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
}

impl Repo {
    /// This checkout. The tool always runs from the tree it was built in,
    /// as `build.py` ran beside its sources.
    pub fn here() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }

    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub(crate) fn private(&self) -> PathBuf {
        self.path("private")
    }
}

/// SHA-256 of a file, read in blocks: toolchain files and images are large.
pub(crate) fn digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
    }
    Ok(wellfed_formats::hex::encode(&hasher.finalize()))
}

fn program(command: &Command) -> String {
    command.get_program().to_string_lossy().into_owned()
}

/// Run a command to completion, refusing a failed exit with its stderr.
pub(crate) fn run(command: &mut Command) -> Result<Output> {
    let output = command.output().map_err(|source| Error::Spawn {
        program: program(command),
        source,
    })?;
    if !output.status.success() {
        return Err(Error::Tool {
            program: program(command),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output)
}

/// A command's stdout as text.
pub(crate) fn text(command: &mut Command) -> Result<String> {
    String::from_utf8(run(command)?.stdout)
        .map_err(|_| Error::Refused("Tool output is not UTF-8".into()))
}

/// `git` in `directory`, stdout trimmed.
pub(crate) fn git(directory: &Path, args: &[&str]) -> Result<String> {
    Ok(text(Command::new("git").args(args).current_dir(directory))?
        .trim()
        .to_string())
}

/// The checked-out commit of a Git worktree.
pub(crate) fn revision(directory: &Path) -> Result<String> {
    git(directory, &["rev-parse", "HEAD"])
}

/// `path` relative to `base`, as the manifest records it.
pub(crate) fn relative(path: &Path, base: &Path) -> Result<String> {
    path.strip_prefix(base)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| Error::Refused(format!("{} is outside {}", path.display(), base.display())))
}

/// An executable on `path` (a `PATH` value), as `shutil.which` finds it.
pub(crate) fn which(name: &str, path: Option<&str>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    path?
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
