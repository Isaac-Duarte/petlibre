//! Repository maintenance tasks, run as `cargo xtask <task>`.
//!
//! `fuzz` runs one cargo-fuzz target for a bounded time, or replays one
//! saved input.
//!
//! `check-repository` checks local documentation links, portable repository
//! inputs, that no Python returns under `src/`, and the source rules (raw
//! output writers only through the HAL's owners, no board card ids), that
//! each diagram in `docs/` still matches its Mermaid, then the server's crate
//! layering from `cargo metadata`. It is a maintenance check, not a secret
//! scanner or a licensing audit.
//!
//! `bless-diagrams` records each diagram pair's hashes in
//! `docs/diagrams.lock`, once its author has checked the SVG and its Mermaid
//! still show the same thing.

#![forbid(unsafe_code)]

mod diagrams;
mod fuzz;
mod layers;
mod repository;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Repository maintenance tasks")]
struct Cli {
    /// Repository checkout to check.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Documentation links, tool dependencies, portable inputs, source rules.
    CheckRepository,
    /// Record each docs diagram's SVG and Mermaid hashes, after checking they match.
    BlessDiagrams,
    /// One bounded libFuzzer run of a network-parser target (needs cargo-fuzz).
    Fuzz {
        #[arg(value_enum)]
        target: fuzz::Target,
        /// How long to fuzz.
        #[arg(long, default_value_t = 60, conflicts_with = "replay")]
        seconds: u32,
        /// Run this one saved input (a crash artifact) instead of fuzzing.
        #[arg(long)]
        replay: Option<PathBuf>,
    },
}

/// Why `check-repository` could not run its checks at all.
#[derive(Debug, thiserror::Error)]
enum CheckError {
    #[error("could not list the checkout")]
    Files(#[from] std::io::Error),
    #[error("could not check the crate layering")]
    Layers(#[from] layers::Error),
}

/// The file checks, then the server's crate layering from `cargo metadata`.
fn check_repository(root: &Path) -> Result<repository::Summary, CheckError> {
    let mut summary = repository::check(root)?;
    summary.errors.extend(layers::check(root)?);
    Ok(summary)
}

/// An error and each of its causes, on one line.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        let _ = write!(text, ": {next}");
        cause = next.source();
    }
    text
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.task {
        Task::CheckRepository => match check_repository(&cli.root) {
            Ok(summary) if summary.errors.is_empty() => {
                println!(
                    "Repository check passed: {} files, {} local documentation links.",
                    summary.files, summary.links
                );
                ExitCode::SUCCESS
            }
            Ok(summary) => {
                eprintln!("{}", summary.errors.join("\n"));
                ExitCode::from(1)
            }
            Err(error) => {
                eprintln!("check-repository: {}", chain(&error));
                ExitCode::from(2)
            }
        },
        Task::BlessDiagrams => {
            let blessed = repository::listed(&cli.root)
                .map_err(|error| vec![error.to_string()])
                .and_then(|names| {
                    diagrams::bless(&cli.root, &names)
                        .map_err(|errors| errors.iter().map(|e| chain(e)).collect())
                });
            match blessed {
                Ok(count) => {
                    println!("Blessed {count} diagrams in {}.", diagrams::LOCK);
                    ExitCode::SUCCESS
                }
                Err(errors) => {
                    eprintln!("{}", errors.join("\n"));
                    ExitCode::from(1)
                }
            }
        }
        Task::Fuzz {
            target,
            seconds,
            replay,
        } => {
            // The run starts in `--root`, so a relative input is resolved here.
            let replay = match replay.map(std::path::absolute).transpose() {
                Ok(replay) => replay,
                Err(error) => {
                    eprintln!("fuzz: {error}");
                    return ExitCode::from(2);
                }
            };
            let run = replay
                .as_deref()
                .map_or(fuzz::Run::Seconds(seconds), fuzz::Run::Replay);
            match fuzz::run(&cli.root, target, run) {
                Ok(status) if status.success() => ExitCode::SUCCESS,
                Ok(status) => {
                    eprintln!(
                        "fuzz: the run ended with {status}; any crash input is under src/fuzz/artifacts/"
                    );
                    ExitCode::from(1)
                }
                Err(error) => {
                    eprintln!("fuzz: {error}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
