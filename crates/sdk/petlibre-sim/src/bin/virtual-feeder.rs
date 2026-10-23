//! `virtual-feeder`: run one replacement-feeder scenario on synthetic
//! hardware and write its report. Exit status: 0 pass, 1 an assertion
//! failed, 2 the scenario or worker was refused.
//!
//! Ported from the former `tools/emulate/virtual-feeder.py`. It drives
//! `virtual-feeder-worker` (beside this binary unless `--worker`) through
//! [`petlibre_sim::run_scenario`]. The report is evidence, so an existing
//! file is never replaced.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::Parser;
use serde_json::Value;

use petlibre_sim::run_scenario;

#[derive(Parser)]
#[command(
    version,
    about = "Deterministic replacement-feeder scenarios; no device or network access"
)]
struct Cli {
    #[arg(long)]
    scenario: PathBuf,
    /// New report file; never overwritten.
    #[arg(long)]
    report: PathBuf,
    /// Repository checkout: sources for the report's identity, private/ for the journal.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// The worker binary (default: beside this one).
    #[arg(long)]
    worker: Option<PathBuf>,
}

fn run(cli: Cli) -> anyhow::Result<bool> {
    let size = fs::metadata(&cli.scenario)
        .with_context(|| format!("Cannot read {}", cli.scenario.display()))?
        .len();
    anyhow::ensure!(size <= 2_000_000, "Scenario exceeds 2 MB");

    let scenario: Value = serde_json::from_slice(&fs::read(&cli.scenario)?)
        .with_context(|| format!("{} is not JSON", cli.scenario.display()))?;

    let worker = cli.worker.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("virtual-feeder-worker")))
            .unwrap_or_else(|| PathBuf::from("virtual-feeder-worker"))
    });

    let report = run_scenario(&scenario, &cli.root, &worker)?;
    // Through `Value`, so the keys are sorted as in every earlier report.
    let text = serde_json::to_string_pretty(&serde_json::to_value(&report)?)?;

    // Exclusive creation prevents accidental evidence replacement.
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&cli.report)
        .with_context(|| format!("Cannot create {}", cli.report.display()))?;
    writeln!(output, "{text}")?;

    let verdict = if report.passed { "PASS" } else { "FAIL" };
    println!("{verdict}: {}", report.name);

    Ok(report.passed)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("virtual-feeder: {error:#}");
            ExitCode::from(2)
        }
    }
}
