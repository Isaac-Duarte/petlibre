//! `wellfed-build`: build AmebaD images locally, without connecting to the
//! feeder. Nothing here opens a serial port or writes a device.
//!
//! Run the executable itself (`target/release/wellfed-build`), not through
//! `cargo run`: cargo sets `RUSTUP_TOOLCHAIN`, which a firmware build refuses
//! as a compiler override.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use wellfed_build::identity::{SourceId, current_inputs, source_id};
use wellfed_build::image::{audit_build, build};
use wellfed_build::toolchain::bootstrap;
use wellfed_build::{Profile, Repo, Result, Variant};

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
    },
    /// Fetch and verify the pinned SDK and Linux toolchain.
    Bootstrap,
    /// Build the unchanged SDK example in a private worktree.
    Baseline,
    /// Build the Rust diagnostic application in a separate worktree.
    Bringup,
    /// Build and audit the diagnostic with stock-derived OTA bank addresses.
    Plaf103,
    /// Build and audit the UART-only SDK mains and the Rust application.
    Plaf103Firstboot {
        #[command(flatten)]
        variant: VariantArgs,
    },
    /// Print a profile's current inputs and their SHA-256 as JSON.
    Inputs {
        #[arg(value_enum)]
        profile: Profile,
    },
    /// Re-run a finished build's audits and print them as JSON, keyed as
    /// its manifest records them.
    Audit {
        #[arg(long, default_value = "private/firmware-build/plaf103-firstboot")]
        build: PathBuf,
    },
}

#[derive(Args)]
#[group(multiple = false)]
struct VariantArgs {
    /// Build the FW16 motor-pulse variant (drives the motor once per cold boot).
    #[arg(long)]
    motion_probe: bool,
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
    let (profile, variant) = match command {
        Command::SourceId { image, variant } => {
            let which = if image {
                SourceId::Image(variant.variant())
            } else {
                SourceId::Bare
            };
            println!("{}", source_id(&repo, which)?);
            return Ok(());
        }
        Command::Inputs { profile } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&current_inputs(&repo, profile)?)?
            );
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
        Command::Bootstrap => (None, None),
        Command::Baseline => (Some(Profile::Baseline), None),
        Command::Bringup => (Some(Profile::Bringup), None),
        Command::Plaf103 => (Some(Profile::Plaf103), None),
        Command::Plaf103Firstboot { variant } => {
            (Some(Profile::Plaf103Firstboot), variant.variant())
        }
    };

    // Build outputs and generated credential headers are private.
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    let toolchain = bootstrap(&repo)?;
    if let Some(profile) = profile {
        build(&repo, profile, &toolchain, variant)?;
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
