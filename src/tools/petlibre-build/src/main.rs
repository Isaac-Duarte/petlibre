//! `petlibre-build`: build AmebaD images locally, without connecting to the
//! feeder. Nothing here opens a serial port or writes a device.
//!
//! Run the executable itself (`target/release/petlibre-build`), not through
//! `cargo run`: cargo sets `RUSTUP_TOOLCHAIN`, which a firmware build refuses
//! as a compiler override.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use petlibre_build::identity::{SourceId, current_inputs, source_id};
use petlibre_build::image::{audit_build, build};
use petlibre_build::toolchain::bootstrap;
use petlibre_build::{Firmware, Profile, Repo, Result, Variant};

#[derive(Parser)]
#[command(about = "Build AmebaD experiments locally, without connecting to the feeder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the first-boot source identity, offline (no SDK bootstrap).
    SourceId {
        /// The identity a build of the chosen variant would embed (with the
        /// update key), so a reviewed build can be reused.
        #[arg(long)]
        image: bool,
        #[command(flatten)]
        variant: VariantArgs,
        #[command(flatten)]
        firmware: FirmwareArgs,
    },
    /// Fetch and verify the pinned SDK and Linux toolchain.
    Bootstrap,
    /// Build the unchanged SDK example in a private worktree.
    Baseline,
    /// Build the Rust diagnostic application in a separate worktree.
    Bringup {
        #[command(flatten)]
        firmware: FirmwareArgs,
    },
    /// Build and audit the diagnostic with stock-derived OTA bank addresses.
    Plaf103 {
        #[command(flatten)]
        firmware: FirmwareArgs,
    },
    /// Build and audit the UART-only SDK mains and the Rust application.
    Plaf103Firstboot {
        #[command(flatten)]
        variant: VariantArgs,
        #[command(flatten)]
        firmware: FirmwareArgs,
    },
    /// Print a profile's current inputs and their SHA-256 as JSON.
    Inputs {
        #[arg(value_enum)]
        profile: Profile,
        #[command(flatten)]
        firmware: FirmwareArgs,
    },
    /// Re-run a finished build's audits and print them as JSON, keyed as
    /// its manifest records them.
    Audit {
        #[arg(long, default_value = "private/firmware-build/plaf103-firstboot")]
        build: PathBuf,
    },
}

#[derive(Args)]
struct FirmwareArgs {
    /// The firmware under `src/firmware/petlibre/` that the KM4 image links.
    #[arg(long, default_value = Firmware::DEFAULT)]
    firmware: String,
}

impl FirmwareArgs {
    fn firmware(&self, repo: &Repo) -> Result<Firmware> {
        Firmware::new(repo, &self.firmware)
    }
}

#[derive(Args)]
#[group(multiple = false)]
struct VariantArgs {
    /// Build the FW16 motor-pulse variant (drives the motor once per cold boot).
    #[arg(long)]
    motion_probe: bool,
    /// Build the FW14 power-input variant (samples the ADC instead of the optics probe).
    #[arg(long)]
    adc_probe: bool,
    /// Build the FW11 trial-test variant (watchdog resets it after startup).
    #[arg(long)]
    trial_reset: bool,
    /// Build the FW11 failed-start variant (its startup diagnostic fails).
    #[arg(long)]
    trial_fail: bool,
    /// Build the speaker variant (plays signed network audio streams).
    #[arg(long)]
    speaker: bool,
}

impl VariantArgs {
    fn variant(&self) -> Option<Variant> {
        [
            (self.motion_probe, Variant::Fw16Pulse),
            (self.adc_probe, Variant::Fw14Adc),
            (self.trial_reset, Variant::Fw11Reset),
            (self.trial_fail, Variant::Fw11Fail),
            (self.speaker, Variant::Speaker),
        ]
        .into_iter()
        .find_map(|(chosen, variant)| chosen.then_some(variant))
    }
}

fn run(command: Command) -> Result<()> {
    let repo = Repo::here()?;
    let (profile, firmware, variant) = match command {
        Command::SourceId {
            image,
            variant,
            firmware,
        } => {
            let which = if image {
                SourceId::Image(variant.variant())
            } else {
                SourceId::Bare
            };
            println!("{}", source_id(&repo, which, &firmware.firmware(&repo)?)?);
            return Ok(());
        }
        Command::Inputs { profile, firmware } => {
            let inputs = current_inputs(&repo, profile, &firmware.firmware(&repo)?)?;
            println!("{}", serde_json::to_string_pretty(&inputs)?);
            return Ok(());
        }
        Command::Audit { build } => {
            let build = repo.root.join(build);
            println!(
                "{}",
                serde_json::to_string_pretty(&audit_build(&repo, &build)?)?
            );
            return Ok(());
        }
        Command::Bootstrap => (None, None, None),
        // The baseline links no Rust; its identity binds no firmware.
        Command::Baseline => (
            Some(Profile::Baseline),
            Some(Firmware::new(&repo, Firmware::DEFAULT)?),
            None,
        ),
        Command::Bringup { firmware } => (
            Some(Profile::Bringup),
            Some(firmware.firmware(&repo)?),
            None,
        ),
        Command::Plaf103 { firmware } => (
            Some(Profile::Plaf103),
            Some(firmware.firmware(&repo)?),
            None,
        ),
        Command::Plaf103Firstboot { variant, firmware } => (
            Some(Profile::Plaf103Firstboot),
            Some(firmware.firmware(&repo)?),
            variant.variant(),
        ),
    };

    // Build outputs and generated credential headers are private.
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    let toolchain = bootstrap(&repo)?;
    if let (Some(profile), Some(firmware)) = (profile, firmware) {
        build(&repo, profile, &toolchain, firmware, variant)?;
    }
    Ok(())
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
