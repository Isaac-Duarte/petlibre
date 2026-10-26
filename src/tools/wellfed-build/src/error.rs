//! The builder's error type: one variant per module that refuses, each
//! wrapping that module's own enum, beside the tool, IO and lower-crate
//! failures every stage shares.
//!
//! Every stop is terminal: the build never retries, and a failed build leaves
//! no manifest describing its outputs. Checks go through
//! [`wellfed_formats::require`], so a refusal is a typed variant a caller or
//! test can match, with the operator's message as its `Display`.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::device::DeviceError;
use crate::environment::EnvironmentError;
use crate::firmware::FirmwareError;
use crate::identity::IdentityError;
use crate::image::ImageError;
use crate::platform::amebad::AmebadError;
use crate::platform::amebad::audit::AuditError;
use crate::platform::amebad::audit::hooks::HookError;
use crate::platform::amebad::audit::layout::LayoutError;
use crate::platform::amebad::audit::mains::MainsError;
use crate::platform::amebad::audit::packaged::PackagedError;
use crate::platform::amebad::audit::park::ParkError;
use crate::platform::amebad::audit::pinmap::PinmapError;
use crate::platform::amebad::audit::stack::StackError;
use crate::platform::amebad::audit::writes::WritesError;
use crate::platform::amebad::credentials::CredentialsError;
use crate::platform::amebad::sdk::SdkError;
use crate::platform::amebad::toolchain::ToolchainError;
use crate::review::ReviewError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A device or firmware directory name that could leave its parent.
    #[error("{kind} names are lowercase letters, digits and dashes: {name:?}")]
    Name { kind: &'static str, name: String },
    /// A tool printed something other than UTF-8 text.
    #[error("Tool output is not UTF-8")]
    NotUtf8,
    /// A path the manifest records relative to a base lies outside it.
    #[error("{} is outside {}", path.display(), base.display())]
    Outside { path: PathBuf, base: PathBuf },
    /// A tool could not be started.
    #[error("Cannot run {program}: {source}")]
    Spawn { program: String, source: io::Error },
    /// A tool ran and exited unsuccessfully.
    #[error("{program} failed ({status}): {stderr}")]
    Tool {
        program: String,
        status: ExitStatus,
        stderr: String,
    },
    /// A build stage failed; its full output stays in the private log.
    #[error("{stage}; see {}", log.display())]
    Logged { stage: &'static str, log: PathBuf },
    /// The offline stock-layout audit refused the reference or the candidate.
    #[error("Offline layout audit failed: {0}")]
    Layout(Box<Error>),
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Firmware(#[from] FirmwareError),
    #[error(transparent)]
    Environment(#[from] EnvironmentError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Image(#[from] ImageError),
    #[error(transparent)]
    Review(#[from] ReviewError),
    #[error(transparent)]
    Amebad(#[from] AmebadError),
    #[error(transparent)]
    Credentials(#[from] CredentialsError),
    #[error(transparent)]
    Sdk(#[from] SdkError),
    #[error(transparent)]
    Toolchain(#[from] ToolchainError),
    #[error(transparent)]
    Audit(#[from] AuditError),
    #[error(transparent)]
    Hooks(#[from] HookError),
    #[error(transparent)]
    LayoutAudit(#[from] LayoutError),
    #[error(transparent)]
    Mains(#[from] MainsError),
    #[error(transparent)]
    Packaged(#[from] PackagedError),
    #[error(transparent)]
    Park(#[from] ParkError),
    #[error(transparent)]
    Pinmap(#[from] PinmapError),
    #[error(transparent)]
    Stack(#[from] StackError),
    #[error(transparent)]
    Writes(#[from] WritesError),
    /// The OTA container or region checks shared with the flash client.
    #[error(transparent)]
    Ota(#[from] wellfed_image::Error),
    /// Signing refused the key, the review or the statement.
    #[error(transparent)]
    Signing(#[from] wellfed_signing::Error),
    /// The factory-loader emulation refused the image or failed.
    #[error(transparent)]
    Emulation(#[from] wellfed_emu::Error),
    /// Reading a shared file or hashing canonical JSON failed.
    #[error(transparent)]
    Formats(#[from] wellfed_formats::Error),
    #[error(transparent)]
    Pattern(#[from] regex::Error),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
