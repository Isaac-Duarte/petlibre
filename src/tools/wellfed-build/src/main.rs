//! `wellfed-build`: build firmware images locally, without connecting to
//! the feeder. Nothing here opens a serial port or writes a device.
//!
//! Run the executable itself (`target/release/wellfed-build`), not through
//! `cargo run`: cargo sets `RUSTUP_TOOLCHAIN`, which a firmware build refuses
//! as a compiler override.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use wellfed_build::catalog::Catalog;
use wellfed_build::identity::{SourceId, current_inputs, source_id};
use wellfed_build::image::{Request, audit_build, build};
use wellfed_build::review::freeze;
use wellfed_build::{Device, Firmware, Profile, Repo, Result, Variant};
use wellfed_formats::review::ImageKind;

#[derive(Parser)]
#[command(about = "Build firmware images locally, without connecting to the feeder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch and verify the device platform's pinned SDK and toolchain.
    Bootstrap {
        #[command(flatten)]
        device: DeviceArgs,
    },
    /// Build a profile of a firmware for a device, and audit it.
    Build {
        #[arg(long, value_enum)]
        profile: Profile,
        #[command(flatten)]
        target: Target,
    },
    /// Print the first-boot source identity, offline (no SDK bootstrap).
    SourceId {
        /// The identity a build of the chosen variant would embed (with the
        /// update key), so a reviewed build can be reused.
        #[arg(long)]
        image: bool,
        #[command(flatten)]
        target: Target,
    },
    /// Print a profile's current inputs and their SHA-256 as JSON.
    Inputs {
        #[arg(long, value_enum)]
        profile: Profile,
        #[command(flatten)]
        target: Target,
    },
    /// Re-run a finished build's audits and print them as JSON, keyed as
    /// its manifest records them.
    Audit {
        #[arg(long, default_value = "private/firmware-build/plaf103/firstboot")]
        build: PathBuf,
    },
    /// Print the devices and firmwares this checkout builds, as JSON.
    List,
    /// Freeze a reviewed image as a new private directory that
    /// `wellfed-flash` installs from, offline (no serial access).
    Review {
        #[arg(value_enum)]
        kind: ReviewKind,
        /// The first-boot build to review (ignored for stock).
        #[arg(long, default_value = "private/firmware-build/plaf103/firstboot")]
        build: PathBuf,
        /// New private output directory.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ReviewKind {
    /// The pinned original stock OTA.
    Stock,
    /// A first-boot build.
    Build,
}

#[derive(Args)]
struct DeviceArgs {
    /// The board under `src/firmware/devices/` (default: the only one).
    #[arg(long)]
    device: Option<String>,
}

#[derive(Args)]
struct Target {
    #[command(flatten)]
    device: DeviceArgs,
    /// The firmware under `src/firmware/wellfed/` that the image links.
    #[arg(long, default_value = "reference")]
    firmware: String,
    /// An image variant the firmware declares (`wellfed-build list`).
    #[arg(long)]
    variant: Option<String>,
}

/// A target, resolved against the checkout.
struct Resolved {
    device: Device,
    firmware: Firmware,
    variant: Option<Variant>,
}

impl Target {
    fn resolve(&self, repo: &Repo) -> Result<Resolved> {
        let device = Device::resolve(repo, self.device.device.as_deref())?;
        let firmware = Firmware::new(repo, &self.firmware)?;
        let variant = self
            .variant
            .as_deref()
            .map(|name| firmware.variant(name))
            .transpose()?;
        Ok(Resolved {
            device,
            firmware,
            variant,
        })
    }
}

fn run(command: Command) -> Result<()> {
    let repo = Repo::here()?;
    match command {
        Command::SourceId { image, target } => {
            let target = target.resolve(&repo)?;
            let which = if image {
                SourceId::Image
            } else {
                SourceId::Bare
            };
            println!(
                "{}",
                source_id(
                    &repo,
                    &target.device,
                    &target.firmware,
                    target.variant.as_ref(),
                    which
                )?
            );
        }
        Command::Inputs { profile, target } => {
            let target = target.resolve(&repo)?;
            let inputs = current_inputs(&repo, &target.device, profile, &target.firmware)?;
            println!("{}", serde_json::to_string_pretty(&inputs)?);
        }
        Command::Audit { build } => {
            let build = repo.root.join(build);
            println!(
                "{}",
                serde_json::to_string_pretty(&audit_build(&repo, &build)?)?
            );
        }
        Command::Review {
            kind,
            build,
            output,
        } => {
            let kind = match kind {
                ReviewKind::Stock => ImageKind::Stock,
                ReviewKind::Build => ImageKind::Build,
            };
            private_outputs();
            let review = freeze(&repo, kind, &build, &output)?;
            let summary = serde_json::json!({
                "image_kind": review.image_kind,
                "ota_sha256": review.ota_sha256,
                "payload_bytes": review.payload_bytes,
            });
            println!("{summary}");
        }
        Command::List => {
            let catalog = Catalog {
                devices: Device::all(&repo)?.iter().map(Device::entry).collect(),
                firmwares: Firmware::all(&repo)?.iter().map(Firmware::entry).collect(),
            };
            println!("{}", serde_json::to_string_pretty(&catalog)?);
        }
        Command::Bootstrap { device } => {
            let device = Device::resolve(&repo, device.device.as_deref())?;
            private_outputs();
            device.bootstrap(&repo)?;
        }
        Command::Build { profile, target } => {
            let target = target.resolve(&repo)?;
            let request = Request::new(
                &target.device,
                profile,
                &target.firmware,
                target.variant.as_ref(),
            )?;

            private_outputs();
            let toolchain = target.device.bootstrap(&repo)?;
            build(&repo, &request, &toolchain)?;
        }
    }
    Ok(())
}

/// Build outputs and generated credential headers are private.
fn private_outputs() {
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
