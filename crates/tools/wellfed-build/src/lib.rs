//! Local AmebaD firmware builds, without connecting to the feeder.
//!
//! The SDK and its Linux toolchain are pinned (`firmware/sdk.lock.json`,
//! `firmware/probe/toolchain.lock.json`) and verified on every run. Each
//! profile builds in its own private SDK worktree, overlaid from the pinned
//! revision; the linked images are then audited ([`audit`]) and described in
//! `manifest.json`, which the frozen review re-checks. The startup identity
//! every image prints is a hash of its exact inputs ([`identity`]), so a
//! review of the same sources can be reused instead of rebuilt.

#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use wellfed_formats::lock::SdkLock;
use sha2::{Digest, Sha256};

pub mod audit;
pub(crate) mod credentials;
pub(crate) mod environment;
mod error;
pub mod identity;
pub mod image;
pub(crate) mod sdk;
pub mod toolchain;

pub use error::{Error, Result, require};
pub(crate) use wellfed_formats::canonical::sha256;

/// The vendor project inside an SDK worktree.
pub const PROJECT: &str = "project/realtek_amebaD_va0_example/GCC-RELEASE";

/// The build profiles, from the unchanged SDK example to the reviewed
/// UART-only first-boot image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    Baseline,
    Bringup,
    Plaf103,
    Plaf103Firstboot,
}

impl Profile {
    /// The manifest and output-directory name; also what the identity binds.
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Bringup => "bringup",
            Self::Plaf103 => "plaf103",
            Self::Plaf103Firstboot => "plaf103-firstboot",
        }
    }

    /// Links the Rust application (everything but the unchanged example).
    pub fn rust(self) -> bool {
        self != Self::Baseline
    }

    /// Uses the stock-derived OTA bank addresses and pad table.
    pub fn model(self) -> bool {
        matches!(self, Self::Plaf103 | Self::Plaf103Firstboot)
    }

    pub fn firstboot(self) -> bool {
        self == Self::Plaf103Firstboot
    }
}

/// The AmebaD's two cores. The SDK names their projects `lp` and `hp`; the
/// manifest and audits call them KM0 and KM4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Core {
    /// KM0, the low-power Cortex-M23.
    Lp,
    /// KM4, the application Cortex-M33.
    Hp,
}

impl Core {
    pub const BOTH: [Self; 2] = [Self::Lp, Self::Hp];

    /// The SDK's directory suffix (`project_lp`, `src_hp`).
    pub fn dir(self) -> &'static str {
        match self {
            Self::Lp => "lp",
            Self::Hp => "hp",
        }
    }

    /// The manifest's and audits' name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lp => "KM0",
            Self::Hp => "KM4",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::BOTH.into_iter().find(|core| core.name() == name)
    }

    /// The Cargo target of the core's firmware archive.
    pub(crate) fn target(self) -> &'static str {
        match self {
            Self::Lp => "thumbv8m.base-none-eabi",
            Self::Hp => "thumbv8m.main-none-eabihf",
        }
    }

    /// The core's feature in its archive crate: `wellfed-sdk-c` for KM0,
    /// the firmware for KM4.
    pub(crate) fn feature(self) -> &'static str {
        match self {
            Self::Lp => "km0",
            Self::Hp => "km4",
        }
    }
}

impl std::fmt::Display for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A firmware crate under `firmwares/`, chosen by its directory name. The
/// set is open (anyone may add a firmware), so this is a checked name rather
/// than an enum. Its Cargo package is `wellfed-fw-<name>`, a KM4 staticlib
/// on `wellfed-sdk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware(String);

impl Firmware {
    /// The firmware a build uses unless told otherwise.
    pub const DEFAULT: &'static str = "reference";

    /// `name`, if this checkout has `firmwares/<name>` with the matching
    /// package.
    pub fn new(repo: &Repo, name: &str) -> Result<Self> {
        require(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            format!("Firmware names are lowercase letters, digits and dashes: {name:?}"),
        )?;
        let firmware = Self(name.to_string());
        let manifest = fs::read_to_string(repo.path(&format!("{}/Cargo.toml", firmware.dir())))
            .map_err(|_| Error::Refused(format!("No firmware named {name} under firmwares/")))?;
        require(
            manifest
                .lines()
                .any(|line| line.trim() == format!("name = \"{}\"", firmware.package())),
            format!("Firmware {name} must be the package {}", firmware.package()),
        )?;
        Ok(firmware)
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// The reference firmware, the only one with image variants and the
    /// FW13 optics probe.
    pub(crate) fn reference(&self) -> bool {
        self.0 == Self::DEFAULT
    }

    /// Its root-relative directory.
    pub(crate) fn dir(&self) -> String {
        format!("firmwares/{}", self.0)
    }

    pub(crate) fn package(&self) -> String {
        format!("wellfed-fw-{}", self.0)
    }

    /// The KM4 archive Cargo writes for the package.
    pub(crate) fn archive(&self) -> String {
        format!("lib{}.a", self.package().replace('-', "_"))
    }
}

/// Explicit opt-in image variants of the first-boot profile. A motion variant
/// drives the motor; it is never built unless named, and its manifest says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// FW16: pulses the motor once per cold boot.
    Fw16Pulse,
    /// FW11: never confirms and lets the watchdog reset it; no motion.
    Fw11Reset,
    /// FW11: fails its startup diagnostic on purpose; no motion.
    Fw11Fail,
    /// The normal image plus a signed network audio stream; no motion.
    Speaker,
}

impl Variant {
    /// The manifest name, which is also the KM4 Cargo feature.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fw16Pulse => "fw16-pulse",
            Self::Fw11Reset => "fw11-reset",
            Self::Fw11Fail => "fw11-fail",
            Self::Speaker => "speaker",
        }
    }

    pub(crate) fn motion_policy(self) -> Option<&'static str> {
        (self == Self::Fw16Pulse).then_some("fw16-pulse-v1")
    }

    pub(crate) fn trial_policy(self) -> Option<&'static str> {
        match self {
            Self::Fw11Reset => Some("fw11-reset-v1"),
            Self::Fw11Fail => Some("fw11-fail-v1"),
            _ => None,
        }
    }
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

    /// The pinned SDK checkout every profile's worktree comes from.
    pub(crate) fn source(&self) -> PathBuf {
        self.path("private/upstream/ameba-rtos-d")
    }

    pub(crate) fn toolchains(&self) -> PathBuf {
        self.path("private/toolchains")
    }

    /// The verified stock image: the pad table and layout reference.
    pub(crate) fn stock(&self) -> PathBuf {
        self.path("private/firmware/PLAF103_OTA_010115.bin")
    }

    pub(crate) fn layout(&self) -> PathBuf {
        self.path("firmware/plaf103-layout.json")
    }

    pub fn output(&self, profile: Profile) -> PathBuf {
        self.private().join("firmware-build").join(profile.name())
    }

    /// `firmware/sdk.lock.json`.
    pub(crate) fn lock(&self) -> Result<SdkLock> {
        Ok(wellfed_formats::load(
            &self.path("firmware/sdk.lock.json"),
        )?)
    }

    /// The pinned toolchain's binutils directory.
    pub fn binutils(&self) -> Result<PathBuf> {
        Ok(self
            .toolchains()
            .join(&self.lock()?.toolchain.directory)
            .join("bin"))
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
