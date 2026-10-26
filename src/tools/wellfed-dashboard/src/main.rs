//! `wellfed-dashboard`: serve the bench's read-only page until Ctrl+C,
//! attached to whatever runs: a bench command or integration run in another
//! terminal, and a `wellfed-stock serve` if one is up.
//!
//! It links no relay driver and no flash client, so it can run beside any
//! bench operation without touching relays, the UART or flash.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser;

use wellfed_dashboard::Listen;
use wellfed_formats::bench::Cancel;

#[derive(Parser)]
#[command(
    version,
    about = "Serve the bench's read-only progress page",
    long_about = "Serve the bench's read-only progress page: the feeder as wellfed-stock last \
        heard it, bench progress, the integration suite's runs and the evidence sessions. It \
        reads private/ under --root and never touches relays, the UART or flash."
)]
struct Cli {
    /// Repository checkout whose ignored private/ holds all evidence.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// The wellfed-flash binary the page reports by hash (default: beside this one).
    #[arg(long)]
    flash_bin: Option<PathBuf>,
    /// Address for the page: loopback, one named interface such as Tailscale,
    /// or a wildcard (`0.0.0.0`, `::`) for every interface.
    #[arg(long, default_value = "127.0.0.1")]
    listen: String,
    #[arg(long, default_value_t = 8780)]
    ui_port: u16,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // The operator's Ctrl+C; the page's streams and server stop on it.
    let stop = Cancel::new();
    let interrupt = stop.clone();
    let _ = ctrlc::set_handler(move || interrupt.cancel());

    match run(cli, stop) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Stopped: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli, stop: Cancel) -> anyhow::Result<()> {
    let root = cli
        .root
        .canonicalize()
        .with_context(|| format!("--root {}", cli.root.display()))?;
    let flash_bin = cli.flash_bin.unwrap_or_else(default_flash_bin);

    let listen = Listen::new(&cli.listen, cli.ui_port)?;
    let page = wellfed_dashboard::start(root, flash_bin, &listen, stop)?;
    println!("Bench page on {} (Ctrl+C to stop)", listen.url());

    page.join()
        .map_err(|_| anyhow::anyhow!("The page's server thread panicked"))
}

/// `wellfed-flash` beside this executable, as the bench finds it.
fn default_flash_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("wellfed-flash")))
        .unwrap_or_else(|| PathBuf::from("wellfed-flash"))
}
