//! `petlibre-audit`: offline evidence audits. Reports go to new files under
//! the checkout's ignored private/; nothing here touches a device.
//!
//! Exit status: 0 pass, 1 the evidence did not pass, 2 refused input.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};

use petlibre_audit::{adc, backup, require, startup, transients, write_report};

#[derive(Parser)]
#[command(
    version,
    about = "Offline audits of PLAF103 evidence; never touches a device"
)]
struct Cli {
    /// Repository checkout whose ignored private/ receives the reports.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct CaptureArgs {
    /// Finalized receive-only capture directory.
    #[arg(long)]
    capture: PathBuf,
    /// The build's manifest.json (or a frozen review's build-manifest.json).
    #[arg(long)]
    manifest: PathBuf,
    /// The OTA_All.bin the manifest names.
    #[arg(long)]
    image: PathBuf,
    /// New report file under private/.
    #[arg(long)]
    output: PathBuf,
}

impl CaptureArgs {
    fn inputs(&self) -> [&Path; 3] {
        [&self.capture, &self.manifest, &self.image]
    }
}

#[derive(Subcommand)]
enum Command {
    /// A diagnostic startup against its source-bound build, under every policy it declares.
    Startup {
        #[command(flatten)]
        args: CaptureArgs,
        /// Required observation after startup (1-180 seconds).
        #[arg(long, default_value_t = 60.0)]
        minimum_seconds: f64,
    },

    /// The FW14 read-only power-input samples after a passing startup.
    Adc {
        #[command(flatten)]
        args: CaptureArgs,
    },

    /// FW30 logic-analyzer cycles from `petlibre-bench transients`: any motor-driver drive.
    Transients {
        /// The session directory with manifest.json and the cycle files.
        #[arg(long)]
        capture: PathBuf,
        /// New report file under private/.
        #[arg(long)]
        output: PathBuf,
    },

    /// Compare two existing full-flash reads.
    Backup {
        first: PathBuf,
        second: PathBuf,
        /// Independently established chip capacity; decimal or 0x-prefixed, never guessed from these files.
        #[arg(long, value_parser = number)]
        capacity_bytes: u64,
        #[arg(long)]
        unit_label: String,
        /// Optional pinned original OTA for bank-body identification; not itself a full backup.
        #[arg(long)]
        stock_reference: Option<PathBuf>,
        /// Optional existing evidence file; recorded by hash, never interpreted as proof.
        #[arg(long)]
        capacity_evidence: Option<PathBuf>,
        #[arg(long)]
        report: PathBuf,
    },
}

fn number(text: &str) -> Result<u64, std::num::ParseIntError> {
    let (digits, radix) = match text.get(..2) {
        Some("0x" | "0X") => (&text[2..], 16),
        Some("0o" | "0O") => (&text[2..], 8),
        Some("0b" | "0B") => (&text[2..], 2),
        _ => (text, 10),
    };
    u64::from_str_radix(&digits.replace('_', ""), radix)
}

/// Exit status for a written report: 0 when it passed, 1 when it did not.
fn status(passed: bool) -> u8 {
    u8::from(!passed)
}

fn run(cli: Cli) -> anyhow::Result<u8> {
    let root = cli
        .root
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", cli.root.display()))?;
    match cli.command {
        Command::Startup {
            args,
            minimum_seconds,
        } => {
            require(
                (1.0..=180.0).contains(&minimum_seconds),
                "Observation minimum must be 1-180 seconds",
            )?;

            let report =
                startup::verify(&args.capture, &args.manifest, &args.image, minimum_seconds)?;
            write_report(&root, &args.output, &report, &args.inputs())?;
            println!("{}", serde_json::to_string(&report)?);
            Ok(status(report.passed()))
        }

        Command::Adc { args } => {
            let report = adc::verify(&args.capture, &args.manifest, &args.image)?;
            write_report(&root, &args.output, &report, &args.inputs())?;
            println!("{}", serde_json::to_string(&report)?);
            Ok(status(report.passed()))
        }

        Command::Transients { capture, output } => {
            let report = transients::verify(&capture)?;
            write_report(&root, &output, &report, &[&capture])?;
            println!("{}", serde_json::to_string(&report.summary)?);
            Ok(status(report.passed()))
        }

        Command::Backup {
            first,
            second,
            capacity_bytes,
            unit_label,
            stock_reference,
            capacity_evidence,
            report: report_path,
        } => {
            let report = backup::audit(&backup::Inputs {
                root: &root,
                first: &first,
                second: &second,
                capacity: capacity_bytes,
                unit_label: &unit_label,
                stock: stock_reference.as_deref(),
                capacity_evidence: capacity_evidence.as_deref(),
            })?;

            let inputs: Vec<&Path> = [
                Some(first.as_path()),
                Some(second.as_path()),
                stock_reference.as_deref(),
                capacity_evidence.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect();

            let written = write_report(&root, &report_path, &report, &inputs)?;
            println!(
                "{}: offline read comparison; report: {}",
                if report.passed() {
                    "PASS"
                } else {
                    "REVIEW REQUIRED"
                },
                written.display()
            );
            println!(
                "This does not verify physical capacity, acquisition independence or recovery."
            );
            Ok(status(report.passed()))
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}
