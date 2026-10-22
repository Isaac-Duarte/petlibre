//! Repository maintenance tasks, run as `cargo xtask <task>`.
//!
//! `check-repository` checks local documentation links, the remaining Python
//! tools' dependencies on each other, portable repository inputs, and the
//! FW08 transport isolation rule. It is a maintenance check, not a secret
//! scanner or a licensing audit.

mod repository;

use std::path::PathBuf;
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
    /// Documentation links, tool dependencies, portable inputs, transport isolation.
    CheckRepository,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.task {
        Task::CheckRepository => match repository::check(&cli.root) {
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
                eprintln!("check-repository: {error}");
                ExitCode::from(2)
            }
        },
    }
}
