//! `virtual-feeder`: run one replacement-feeder scenario on synthetic
//! hardware and write its report. Exit status: 0 pass, 1 an assertion
//! failed, 2 the scenario or worker was refused.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use serde_json::Value;

use petlibre_sim::{HarnessError, run_scenario};

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

fn run(cli: Cli) -> Result<bool, HarnessError> {
    let error = |e: std::io::Error| HarnessError(e.to_string());
    if fs::metadata(&cli.scenario).map_err(error)?.len() > 2_000_000 {
        return Err(HarnessError("Scenario exceeds 2 MB".into()));
    }
    let scenario: Value = serde_json::from_slice(&fs::read(&cli.scenario).map_err(error)?)
        .map_err(|e| HarnessError(e.to_string()))?;
    let worker = cli.worker.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("virtual-feeder-worker")))
            .unwrap_or_else(|| PathBuf::from("virtual-feeder-worker"))
    });
    let report = run_scenario(&scenario, &cli.root, &worker)?;
    // Exclusive creation prevents accidental evidence replacement.
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&cli.report)
        .map_err(error)?;
    writeln!(
        output,
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| HarnessError(e.to_string()))?
    )
    .map_err(error)?;
    let passed = report["passed"] == true;
    println!(
        "{}: {}",
        if passed { "PASS" } else { "FAIL" },
        report["name"].as_str().unwrap_or("")
    );
    Ok(passed)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("virtual-feeder: {error}");
            ExitCode::from(2)
        }
    }
}
