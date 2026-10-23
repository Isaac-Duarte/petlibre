//! Local AmebaD firmware builds, without connecting to the feeder.
//!
//! The SDK and its Linux toolchain are pinned (`firmware/sdk.lock.json`,
//! `firmware/probe/toolchain.lock.json`) and verified on every run. Each
//! profile builds in its own private SDK worktree, overlaid from the pinned
//! revision; the linked images are then audited ([`audit`]) and described in
//! `manifest.json`, which the frozen review re-checks. The startup identity
//! every image prints is a hash of its exact inputs ([`identity`]), so a
//! review of the same sources can be reused instead of rebuilt.

// The manifest is one `json!` literal.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use petlibre_formats::lock::SdkLock;
use sha2::{Digest, Sha256};

pub mod audit;
pub mod credentials;
pub mod environment;
pub mod identity;
pub mod image;
pub mod sdk;
pub mod toolchain;

pub use petlibre_flash::error::{Error, Result, require};

/// The vendor project inside an SDK worktree.
pub const PROJECT: &str = "project/realtek_amebaD_va0_example/GCC-RELEASE";

/// The build profiles, from the unchanged SDK example to the reviewed
/// UART-only first-boot image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Baseline,
    Bringup,
    Plaf103,
    Plaf103Firstboot,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Bringup => "bringup",
            Self::Plaf103 => "plaf103",
            Self::Plaf103Firstboot => "plaf103-firstboot",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::Baseline,
            Self::Bringup,
            Self::Plaf103,
            Self::Plaf103Firstboot,
        ]
        .into_iter()
        .find(|profile| profile.name() == name)
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

    pub fn motion_policy(self) -> Option<&'static str> {
        (self == Self::Fw16Pulse).then_some("fw16-pulse-v1")
    }

    pub fn trial_policy(self) -> Option<&'static str> {
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn firmware(&self) -> PathBuf {
        self.path("firmware")
    }

    pub fn private(&self) -> PathBuf {
        self.path("private")
    }

    /// The pinned SDK checkout every profile's worktree comes from.
    pub fn source(&self) -> PathBuf {
        self.path("private/upstream/ameba-rtos-d")
    }

    pub fn toolchains(&self) -> PathBuf {
        self.path("private/toolchains")
    }

    /// The verified stock image: the pad table and layout reference.
    pub fn stock(&self) -> PathBuf {
        self.path("private/firmware/PLAF103_OTA_010115.bin")
    }

    pub fn layout(&self) -> PathBuf {
        self.path("firmware/plaf103-layout.json")
    }

    pub fn output(&self, profile: Profile) -> PathBuf {
        self.private().join("firmware-build").join(profile.name())
    }

    /// `firmware/sdk.lock.json`.
    pub fn lock(&self) -> Result<SdkLock> {
        Ok(petlibre_formats::load(
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
pub fn digest(path: &Path) -> Result<String> {
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
    Ok(petlibre_formats::hex::encode(&hasher.finalize()))
}

pub fn sha256(data: &[u8]) -> String {
    petlibre_formats::canonical::sha256(data)
}

/// Run a command to completion, refusing a failed exit with its stderr.
pub fn run(command: &mut Command) -> Result<Output> {
    let output = command.output().map_err(|error| {
        Error::Refused(format!(
            "Cannot run {}: {error}",
            command.get_program().to_string_lossy()
        ))
    })?;
    require(
        output.status.success(),
        format!(
            "{} failed ({}): {}",
            command.get_program().to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    )?;
    Ok(output)
}

/// A command's stdout as text.
pub fn text(command: &mut Command) -> Result<String> {
    String::from_utf8(run(command)?.stdout)
        .map_err(|_| Error::Refused("Tool output is not UTF-8".into()))
}

/// `git` in `directory`, stdout trimmed.
pub fn git(directory: &Path, args: &[&str]) -> Result<String> {
    Ok(text(Command::new("git").args(args).current_dir(directory))?
        .trim()
        .to_string())
}

/// The checked-out commit of a Git worktree.
pub fn revision(directory: &Path) -> Result<String> {
    git(directory, &["rev-parse", "HEAD"])
}

/// `path` relative to `base`, as the manifest records it.
pub fn relative(path: &Path, base: &Path) -> Result<String> {
    path.strip_prefix(base)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| Error::Refused(format!("{} is outside {}", path.display(), base.display())))
}

/// An executable on `path` (a `PATH` value), as `shutil.which` finds it.
pub fn which(name: &str, path: Option<&str>) -> Option<PathBuf> {
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
