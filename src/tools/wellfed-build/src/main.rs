//! `wellfed-build`: build firmware images locally, without connecting to
//! the feeder, and sign frozen reviews for updates. Nothing here opens a
//! serial port or writes a device.
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
use wellfed_build::signing::{self, Frozen, Key, Motion};
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
    /// Create the bench update-signing key; never replaces one. Prints the
    /// public key that first-boot builds embed. A public release needs an
    /// offline release key instead, which this tool never creates.
    Keygen {
        #[arg(long, default_value = signing::KEY_PATH)]
        output: PathBuf,
    },
    /// Sign a frozen review's image as a version 2 update manifest for the
    /// device API, refusing an image that differs from its review.
    Sign {
        #[arg(long, default_value = signing::KEY_PATH)]
        key: PathBuf,
        /// A directory `review` froze: image.bin, review.json and, for a
        /// build, build-manifest.json.
        #[arg(long)]
        review: PathBuf,
        /// Must exceed the installed image's version.
        #[arg(long)]
        version: u32,
        /// Sign a motion image: the exact startup motion policy its review
        /// declares, with the owner watching the feeder.
        #[arg(long)]
        attended_motion: Option<String>,
        /// New file for the signed manifest.
        #[arg(long)]
        output: PathBuf,
    },
    /// Sign a downgrade statement: the feeder with this identity key may
    /// install this version 2 manifest while its floor is `floor`. Stock
    /// always needs one, as does any version at or below the floor.
    Downgrade {
        #[arg(long, default_value = signing::KEY_PATH)]
        key: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        /// The feeder's identity key, 64 hex characters (mDNS TXT `id=`).
        #[arg(long)]
        device: String,
        /// The feeder's `min_version` now (`wellfed-bench ota` reads it).
        #[arg(long)]
        floor: u32,
        #[arg(long)]
        output: PathBuf,
    },
    /// Sign a key statement retiring an update key id for good.
    Revoke {
        #[arg(long, default_value = signing::KEY_PATH)]
        key: PathBuf,
        /// The key id to retire, 16 hex characters.
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Sign a key statement filling the firmware's release slot with a key.
    AddKey {
        #[arg(long, default_value = signing::KEY_PATH)]
        key: PathBuf,
        /// The Ed25519 public key to add, 64 hex characters.
        #[arg(long)]
        public: String,
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
    #[arg(long, default_value = "qa")]
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
        Command::Keygen { output } => {
            // Paths are the checkout's, like every other command's, so the
            // default key lands where builds read it from any directory.
            let output = repo.root.join(output);
            let public = signing::keygen(&output)?;
            let id = Key::load(&output)?.id();
            println!(
                "Wrote bench key {}; public key {public}, key id {}",
                output.display(),
                wellfed_formats::hex::encode(&id)
            );
        }
        Command::Sign {
            key,
            review,
            version,
            attended_motion,
            output,
        } => {
            let motion = attended_motion
                .as_deref()
                .map_or(Motion::Refused, Motion::Attended);
            let (key, output) = (repo.root.join(key), repo.root.join(output));
            let frozen = Frozen::open(&repo.root, &repo.root.join(review))?;
            let key = Key::load(&key)?;
            let manifest = frozen.offer(&key, version, motion)?;
            wellfed_flash::evidence::write_new(&output, &manifest)?;
            println!(
                "Signed version {version} of build {} in {}",
                frozen.build_id(),
                output.display()
            );
        }
        Command::Downgrade {
            key,
            manifest,
            device,
            floor,
            output,
        } => {
            let device = wellfed_formats::hex::decode::<32>(&device).ok_or_else(|| {
                wellfed_build::Error::Refused("--device is not 64 hex characters".into())
            })?;
            let manifest = std::fs::read(repo.root.join(manifest))?;
            let key = Key::load(&repo.root.join(key))?;
            let statement = key.downgrade(&manifest, &device, floor)?;
            let output = repo.root.join(output);
            wellfed_flash::evidence::write_new(&output, &statement)?;
            println!(
                "Signed a downgrade at floor {floor} in {}",
                output.display()
            );
        }
        Command::Revoke {
            key,
            key_id,
            output,
        } => {
            let id = wellfed_formats::hex::decode::<8>(&key_id).ok_or_else(|| {
                wellfed_build::Error::Refused("--key-id is not 16 hex characters".into())
            })?;
            let statement = Key::load(&repo.root.join(key))?.revoke(&id);
            let output = repo.root.join(output);
            wellfed_flash::evidence::write_new(&output, &statement)?;
            println!("Signed the retirement of {key_id} in {}", output.display());
        }
        Command::AddKey {
            key,
            public,
            output,
        } => {
            let added = wellfed_formats::hex::decode::<32>(&public).ok_or_else(|| {
                wellfed_build::Error::Refused("--public is not 64 hex characters".into())
            })?;
            let statement = Key::load(&repo.root.join(key))?.add_key(&added);
            let output = repo.root.join(output);
            wellfed_flash::evidence::write_new(&output, &statement)?;
            println!(
                "Signed adding key id {} in {}",
                wellfed_formats::hex::encode(&wellfed_core::ota::offer::key_id(&added)),
                output.display()
            );
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
