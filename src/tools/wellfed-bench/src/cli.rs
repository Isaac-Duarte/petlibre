//! `wellfed-bench`'s command line: `serve` runs the bench server in the
//! foreground, and every hardware command is its client.
//!
//! - `serve` takes the bench lease (`private/bench.lease`) and the relay
//!   board for its life and answers on `private/bench.sock`; `--ui` serves
//!   the read-only progress page beside it.
//! - A hardware command (`boot`, `flash-execute`, `ota`, `itest`, …) posts
//!   its job, prints its output as it comes, and exits with its code. Ctrl+C
//!   asks the server to cancel the job and keeps printing its cleanup; a
//!   second Ctrl+C detaches, and `attach ID` follows the job again.
//! - With no server running, only `status` (and `status --break-lease`)
//!   runs: it reports the lease and reads the relay board itself, which is
//!   the way back after a killed server. Every other command refuses and
//!   names `wellfed-bench serve`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::page;
use clap::{Args, Parser, Subcommand};
use tokio::sync::mpsc;
use wellfed_bench_server::{
    Client, ClientError, Followed, Interrupt, JobCommand, JobSpec, Options, Server, ToolPaths,
};
use wellfed_formats::bench::catalog;
use wellfed_formats::bench::lease::{self, Liveness};
use wellfed_formats::bench::live::{BenchState, JobId, Phase, Uart};
use wellfed_formats::bench::{CONFIG, Config};
use wellfed_relay::{AtBoard, Relay};
use wellfed_rig::Cancel;
use wellfed_rig::commands::{Bench, UartWhenOff, check_name, seconds_arg};
use wellfed_rig::tools::{
    Children, Processes, default_build_bin, default_flash_bin, default_stock_bin,
};

/// A client that detached from a job it left running exits with this, as
/// an interrupted command does.
const DETACHED: u8 = 130;

#[derive(Parser)]
#[command(
    version,
    about = "Relay bench for the PLAF103 research unit",
    long_about = "Relay bench for the PLAF103 research unit.\n\n\
        `wellfed-bench serve` is the bench: it holds the bench lease and the relay board, and runs \
        one job at a time. Every other command but `status` is its client. Relay 1 switches the \
        feeder's USB 5V; relay 2 holds feeder TX to GND through the 4.7 kΩ strap. Serial work is \
        delegated unchanged to wellfed-flash, so its pinned checks, plan hashes and no-retry rules \
        apply. A failed step stops without retry; a stopped update is never followed by a boot. \
        Ports and evidence paths come from the ignored private/bench.json; see \
        docs/devices/plaf103/bench.md. Running a command is a physical bench operation.",
    arg_required_else_help = true
)]
struct Cli {
    /// Repository checkout whose ignored private/ holds all evidence.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    /// Power-off time before a cold boot, in seconds.
    #[arg(long, global = true, default_value_t = 2.0)]
    off_seconds: f64,
    /// What the UART lines (relays 3 and 4) do while the feeder is off.
    #[arg(long, global = true, value_enum, default_value_t = UartWhenOff::Isolated)]
    uart_when_off: UartWhenOff,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the bench lease, the server and its job, relay states and port
    /// presence; with no server, read the relay board directly.
    Status {
        /// First free a bench whose lease outlived its holder (a killed
        /// run); a live holder's lease is never broken.
        #[arg(long)]
        break_lease: bool,
    },
    /// Run the bench server in the foreground until Ctrl+C: it holds the
    /// lease and the relay board, and runs one job at a time.
    Serve(ServeArgs),
    /// Follow a job of the running server again, from its first kept line.
    Attach {
        /// The job's number, as its command printed it.
        id: JobId,
    },
    #[command(flatten)]
    Job(JobCommand),
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// The wellfed-flash binary every job runs (default: beside this one).
    #[arg(long)]
    flash_bin: Option<PathBuf>,
    /// The wellfed-stock binary for `itest` and `stock-ota` (default: beside this one).
    #[arg(long)]
    stock_bin: Option<PathBuf>,
    /// The wellfed-build binary for `--build` (default: beside this one).
    #[arg(long)]
    build_bin: Option<PathBuf>,

    /// Serve the read-only progress page beside the server.
    #[arg(long)]
    ui: bool,
    /// Address for the page: loopback, or one named interface such as Tailscale.
    #[arg(long, default_value = "127.0.0.1", requires = "ui")]
    listen: String,
    #[arg(long, default_value_t = 8780, requires = "ui")]
    ui_port: u16,
}

/// Parse the command line and run it.
pub(crate) fn main() -> ExitCode {
    let cli = Cli::parse();
    // Evidence, status and the socket are private.
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));

    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Stopped: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<u8> {
    let root = cli
        .root
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("--root {}: {error}", cli.root.display()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    match cli.command {
        // The catalog needs no bench.
        Command::Job(JobCommand::Itest(args)) if args.list => {
            for line in catalog::listing() {
                wellfed_rig::say!("{line}");
            }
            Ok(0)
        }
        Command::Status { break_lease } => status(&root, break_lease, &runtime),
        Command::Serve(args) => serve(&root, args, &runtime),
        Command::Attach { id } => runtime.block_on(follow(&Client::new(&root), id)),
        Command::Job(command) => {
            // Refused here as the server would, before anything is sent.
            if let Some(name) = command.name() {
                check_name(name)?;
            }
            seconds_arg("--off-seconds", cli.off_seconds)?;
            let spec = JobSpec {
                command,
                off_seconds: cli.off_seconds,
                uart_when_off: cli.uart_when_off,
            };
            runtime.block_on(submit(&Client::new(&root), &spec))
        }
    }
}

/// Post the job and follow it to its end.
async fn submit(client: &Client, spec: &JobSpec) -> anyhow::Result<u8> {
    let id = client.submit(spec).await?;
    wellfed_rig::say!(
        "Job {id}: {} (Ctrl+C cancels it; a second Ctrl+C leaves it running)",
        spec.command.describe()
    );
    follow(client, id).await
}

/// Print job `id`'s output until it ends: its code, or [`DETACHED`].
async fn follow(client: &Client, id: JobId) -> anyhow::Result<u8> {
    let (interrupts, mut received) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut asked = Interrupt::Cancel;
        while tokio::signal::ctrl_c().await.is_ok() {
            if interrupts.send(asked).is_err() {
                return;
            }
            asked = Interrupt::Detach;
        }
    });

    match client
        .follow(id, |line| wellfed_rig::say!("{line}"), &mut received)
        .await?
    {
        Followed::Ended(end) => {
            if let Some(error) = end.error {
                eprintln!("Stopped: {error}");
            }
            Ok(end.code)
        }
        Followed::Detached => {
            wellfed_rig::say!(
                "Left job {id} running; `wellfed-bench attach {id}` follows it again"
            );
            Ok(DETACHED)
        }
    }
}

/// Run the server until Ctrl+C or SIGTERM.
fn serve(root: &Path, args: ServeArgs, runtime: &tokio::runtime::Runtime) -> anyhow::Result<u8> {
    let tools = ToolPaths {
        flash_bin: args.flash_bin.unwrap_or_else(default_flash_bin),
        stock_bin: args.stock_bin.unwrap_or_else(default_stock_bin),
        build_bin: args.build_bin.unwrap_or_else(default_build_bin),
    };
    if args.ui {
        page::available()?;
    }
    let options = Options {
        root: root.to_path_buf(),
        tools: tools.clone(),
    };
    let server = Server::open(options, |config| {
        Ok(Box::new(AtBoard::open(&config.relay_port)?))
    })?;

    let stop_page = Cancel::new();
    let page = if args.ui {
        let page = page::Page {
            root,
            flash_bin: &tools.flash_bin,
            address: &args.listen,
            port: args.ui_port,
        };
        Some(page.start(&stop_page, server.store())?)
    } else {
        None
    };

    let relays: Vec<String> = server
        .store()
        .read()
        .relays
        .iter()
        .map(|(channel, relay)| format!("{channel} {}", on_off(relay.on)))
        .collect();
    server.say(format!(
        "Bench server on {}, holding the bench lease; relays {} (Ctrl+C stops it)",
        server.socket().display(),
        relays.join(", ")
    ));
    let served = runtime.block_on(server.run(shutdown()));

    stop_page.cancel();
    if let Some(page) = page {
        let _ = page.join();
    }
    served?;
    Ok(0)
}

/// The operator's Ctrl+C, or a SIGTERM.
async fn shutdown() {
    let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

/// `status`: the lease first, then the server's view, or with no server the
/// relay board read directly.
fn status(root: &Path, break_lease: bool, runtime: &tokio::runtime::Runtime) -> anyhow::Result<u8> {
    report_lease(root, break_lease)?;

    match runtime.block_on(Client::new(root).state()) {
        Ok(state) => {
            report_state(&state);
            report_ports(root);
            Ok(0)
        }
        Err(ClientError::NoServer { .. }) => {
            wellfed_rig::say!("Server: none (`wellfed-bench serve` starts one)");
            Ok(direct_status(root)?)
        }
        Err(error) => Err(error.into()),
    }
}

/// Who holds the bench. With `--break-lease`, a lease whose holder is gone
/// is removed first; anything else refuses. Printed before the relay board
/// opens, so it shows even while a holder keeps the board's port.
fn report_lease(root: &Path, break_lease: bool) -> anyhow::Result<()> {
    if break_lease {
        let holder = lease::break_stale(root)?;
        wellfed_rig::say!("Lease: broke the stale lease of {holder}");
    }

    match lease::inspect(root) {
        Ok(None) => wellfed_rig::say!("Lease: free"),
        Ok(Some((holder, Liveness::Alive))) => wellfed_rig::say!("Lease: held by {holder}"),
        Ok(Some((holder, Liveness::Unknown))) => {
            wellfed_rig::say!("Lease: held by {holder}, on another host");
        }
        Ok(Some((holder, Liveness::Gone))) => wellfed_rig::say!(
            "Lease: left by {holder}, which is gone (a killed run?); check the relays below, \
             then `status --break-lease` frees the bench"
        ),
        // An unreadable lease is reported; the relays are still worth showing.
        Err(error) => wellfed_rig::say!("Lease: {error}"),
    }
    Ok(())
}

fn report_state(state: &BenchState) {
    let phase = match state.server.phase {
        Phase::Serving => "serving",
        Phase::Stopping => "stopping",
    };
    wellfed_rig::say!("Server: pid {}, {phase}", state.server.pid);

    match &state.job {
        None => wellfed_rig::say!("Job: none yet"),
        Some(job) => {
            let how = match &job.end {
                None => "running".to_string(),
                Some(end) => format!("ended with code {}", end.code),
            };
            wellfed_rig::say!("Job {}: {} ({how})", job.id, job.label);
            if job.end.is_none() && !job.upcoming.is_empty() {
                wellfed_rig::say!("  next: {}", job.upcoming.join(", "));
            }
        }
    }
    for child in &state.activity {
        wellfed_rig::say!("  child {}: {:?} {}", child.pid, child.kind, child.label);
    }

    match &state.uart {
        Uart::Idle => wellfed_rig::say!("UART: idle"),
        Uart::Capturing { path } => wellfed_rig::say!(
            "UART: capturing{}",
            path.as_deref()
                .map(|p| format!(" into {p}"))
                .unwrap_or_default()
        ),
        Uart::Loader => wellfed_rig::say!("UART: in a flash client's loader session"),
        Uart::Writing { done, total } => wellfed_rig::say!(
            "UART: WRITING FLASH ({done}/{total} sectors verified); do not touch the bench"
        ),
    }
    for (channel, relay) in &state.relays {
        wellfed_rig::say!("Relay {channel}: {}", on_off(relay.on));
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Whether the configured ports exist, as the server's own status cannot
/// show them for a config it may have read long ago.
fn report_ports(root: &Path) {
    let Ok(config) = wellfed_formats::load::<Config>(&root.join(CONFIG)) else {
        return;
    };
    for (key, port) in [
        ("relay_port", &config.relay_port),
        ("uart_port", &config.uart_port),
    ] {
        let present = Path::new(port).exists();
        wellfed_rig::say!("{key}: {}", if present { "present" } else { "missing" });
    }
}

/// With no server: read the relays and ports as the bench always could, so
/// a killed server's bench can be looked at before it is freed.
fn direct_status(root: &Path) -> wellfed_rig::Result<u8> {
    let config: Config = wellfed_rig::load(root, Path::new(CONFIG))?;
    let relay = Relay::open(&config.relay_port)?;
    let children = Children::new();
    let cancel = Cancel::new();
    let tools = Processes::new(
        root.to_path_buf(),
        default_flash_bin(),
        default_stock_bin(),
        default_build_bin(),
        cancel.clone(),
        children.clone(),
    );

    let mut bench = Bench {
        root: root.to_path_buf(),
        config,
        relay,
        status: wellfed_formats::bench::progress::Status::disabled(),
        tools: Box::new(tools),
        children,
        cancel,
        sleep: wellfed_rig::sleep,
        off_time: std::time::Duration::ZERO,
        uart_when_off: UartWhenOff::Isolated,
    };
    bench.status_report()
}

#[cfg(test)]
mod tests;
