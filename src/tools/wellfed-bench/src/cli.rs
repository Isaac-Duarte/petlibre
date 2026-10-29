//! `wellfed-bench`'s command line: `serve` runs the bench server in the
//! foreground, and every hardware command is its client.
//!
//! - `serve` takes the bench lease (`private/bench.lease`) and the relay
//!   board for its life and answers on `private/bench.sock`; `--ui` serves
//!   the read-only progress page beside it.
//! - A hardware command (`boot`, `flash-execute`, `ota`, `itest`, …) posts
//!   its job, prints its output as it comes, and exits with its code. Ctrl+C
//!   asks the server to cancel the job and keeps printing its cleanup;
//!   during a UART write only a second Ctrl+C within 5 s does. Once the
//!   cancel is taken, another Ctrl+C detaches, and `attach ID` follows the
//!   job again.
//! - `answer JOB SEQ done|yes|no` answers the question a job (`install`)
//!   asks, once; the page answers it too.
//! - With no server running, only `status` (and `status --break-lease`)
//!   runs: it reports the lease and reads the relay board itself, which is
//!   the way back after a killed server. Every other command refuses and
//!   names `wellfed-bench serve`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{mcp, note, orphans, page, tui};
use clap::{Args, Parser, Subcommand};
use tokio::sync::mpsc;
use wellfed_bench_server::api::{Seed, SeededFrom};
use wellfed_bench_server::stopping::Ask;
use wellfed_bench_server::{
    CONSOLE_QUEUE, Client, ClientError, ClockSync, Console, DeviceOptions, Followed, Interrupt,
    JobCommand, JobOptions, Options, Server, ToolPaths,
};
use wellfed_formats::bench::banks::{Bank, Check, Content, Entry};
use wellfed_formats::bench::catalog;
use wellfed_formats::bench::lease::{self, Liveness};
use wellfed_formats::bench::live::{BenchState, JobId, Phase, Session, Uart};
use wellfed_formats::bench::prompt::Answer;
use wellfed_formats::bench::rendezvous;
use wellfed_formats::bench::{CONFIG, Config};
use wellfed_relay::{AtBoard, Board, Relay};
use wellfed_rig::Cancel;
use wellfed_rig::commands::{Bench, UartWhenOff, check_name, seconds_arg};
use wellfed_rig::tools::{
    Children, Processes, default_build_bin, default_flash_bin, default_stock_bin,
    default_wellfed_bin,
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
    /// The server parses these again from the words a job posts.
    #[command(flatten)]
    options: JobOptions,

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
    /// Answer the question a running job asks (`install`), as the page
    /// and the console show it with its number.
    Answer {
        /// The job's number.
        job: JobId,
        /// The question's number, as the job printed it.
        seq: u32,
        #[arg(value_enum)]
        answer: Reply,
    },
    /// The bank ledger: which build the server believes each flash bank
    /// holds, and whether the feeder's own reports agree.
    Banks(BanksArgs),
    /// Record the operator's step, next steps, timers and evidence on the
    /// running server, for the page and the console.
    Note(note::NoteArgs),
    /// Serve the bench's tools to an agent over the Model Context Protocol
    /// on stdio, as a client of the running server: status, waits, notes,
    /// device reads and every hardware job, with what needs a person at
    /// the bench refused.
    Mcp,
    #[command(flatten)]
    Job(JobCommand),
}

/// An answer, as the command line spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Reply {
    /// The instruction is done.
    Done,
    Yes,
    No,
}

impl From<Reply> for Answer {
    fn from(reply: Reply) -> Self {
        match reply {
            Reply::Done => Self::Done,
            Reply::Yes => Self::Yes,
            Reply::No => Self::No,
        }
    }
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
    /// The wellfed binary for `install` (default: beside this one).
    #[arg(long)]
    wellfed_bin: Option<PathBuf>,

    /// Serve the read-only progress page beside the server.
    #[arg(long)]
    ui: bool,
    /// Address for the page: loopback, or one named interface such as Tailscale.
    #[arg(long, default_value = "127.0.0.1", requires = "ui")]
    listen: String,
    #[arg(long, default_value_t = 8780, requires = "ui")]
    ui_port: u16,
    /// The feeder's device API address, rather than finding it over mDNS
    /// or the hotspot's neighbour table (a simulated feeder, say).
    #[arg(long)]
    feeder_address: Option<std::net::SocketAddr>,
    /// Leave the feeder's clock as it is, rather than set it when it is
    /// invalid or off by more than 2 s.
    #[arg(long)]
    no_clock_sync: bool,
    /// Print the console as plain lines rather than the full-screen view,
    /// which is also what happens without a terminal.
    #[arg(long)]
    plain: bool,

    /// A scripted relay board in place of `relay_port`, for the page's
    /// end-to-end tests; only a binary built with `fake-board` has it.
    #[cfg(feature = "fake-board")]
    #[arg(long, hide = true)]
    fake_board: bool,
}

#[derive(Debug, Args)]
struct BanksArgs {
    /// Seed the ledger from this dump session's inspection, or, when it
    /// names no build, from --bank and --build.
    #[arg(long, value_name = "DUMP_SESSION")]
    seed: Option<String>,
    #[arg(long, requires = "seed", requires = "build")]
    bank: Option<u8>,
    /// The build's startup identity, hex.
    #[arg(long, requires = "seed", requires = "bank")]
    build: Option<String>,
}

/// Parse the command line and run it.
pub(crate) fn main() -> ExitCode {
    // The words after the program's name are what a job posts.
    let argv: Result<Vec<String>, _> = std::env::args_os()
        .skip(1)
        .map(std::ffi::OsString::into_string)
        .collect();
    let Ok(argv) = argv else {
        eprintln!("Stopped: an argument is not UTF-8");
        return ExitCode::from(2);
    };
    let cli = Cli::parse_from(std::iter::once("wellfed-bench".to_string()).chain(argv.clone()));
    // Evidence and status are private.
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));

    let ran = run(cli, &argv);
    // A server's console goes back to the terminal before its last line.
    wellfed_bench_server::restore_output();
    match ran {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Stopped: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli, argv: &[String]) -> anyhow::Result<u8> {
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
        Command::Serve(args) => serve(&root, &args, &runtime),
        Command::Banks(args) => runtime.block_on(banks(&Client::new(&root), args)),
        Command::Answer { job, seq, answer } => runtime.block_on(async {
            let answer = Answer::from(answer);
            Client::new(&root).answer(job, seq, answer).await?;
            wellfed_rig::say!("Job {job}: answered {answer} to question {seq}");
            Ok(0)
        }),
        Command::Note(args) => runtime.block_on(note::note(&Client::new(&root), &root, args)),
        Command::Mcp => runtime.block_on(mcp::serve(&root)),
        Command::Attach { id } => runtime.block_on(async {
            let mut interrupts = interrupts()?;
            follow(&Client::new(&root), id, &mut interrupts).await
        }),
        Command::Job(command) => {
            // Refused here as the server would, before anything is sent.
            if let Some(name) = command.name() {
                check_name(name)?;
            }
            seconds_arg("--off-seconds", cli.options.off_seconds)?;
            runtime.block_on(submit(&Client::new(&root), &root, argv, &command))
        }
    }
}

/// The operator's interrupts from now on, each a Ctrl+C: the client asks
/// for a cancel (twice within 5 s during a UART write) and, once it is
/// accepted, detaches on the next. Taken before a job is posted, so a
/// Ctrl+C that comes while the server answers cancels the job once its
/// number arrives.
fn interrupts() -> anyhow::Result<mpsc::UnboundedReceiver<Interrupt>> {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let (interrupts, received) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while signal.recv().await.is_some() {
            if interrupts.send(Interrupt::Cancel).is_err() {
                return;
            }
        }
    });
    Ok(received)
}

/// The words a job posts: the command line without `--root`, which goes
/// beside them, canonical, for the server to check against its own.
fn job_words(argv: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    let mut rest = argv.iter();
    while let Some(word) = rest.next() {
        if word == "--root" {
            rest.next();
        } else if !word.starts_with("--root=") {
            words.push(word.clone());
        }
    }
    words
}

/// Post the job's words and follow it to its end.
async fn submit(
    client: &Client,
    root: &Path,
    argv: &[String],
    command: &JobCommand,
) -> anyhow::Result<u8> {
    let mut interrupts = interrupts()?;
    let id = client.submit(root, &job_words(argv)).await?;
    wellfed_rig::say!(
        "Job {id}: {} (Ctrl+C cancels it, twice within 5 s during a UART write; once cancelled, another Ctrl+C leaves it running)",
        command.describe()
    );
    follow(client, id, &mut interrupts).await
}

/// Print job `id`'s output until it ends: its code, or [`DETACHED`].
async fn follow(
    client: &Client,
    id: JobId,
    received: &mut mpsc::UnboundedReceiver<Interrupt>,
) -> anyhow::Result<u8> {
    match client
        .follow(id, |line| wellfed_rig::say!("{line}"), received)
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

/// Run the server until Ctrl+C or SIGTERM, held back during a UART write
/// as `wellfed_bench_server::stopping` says.
fn serve(root: &Path, args: &ServeArgs, runtime: &tokio::runtime::Runtime) -> anyhow::Result<u8> {
    let tools = ToolPaths {
        flash_bin: args.flash_bin.clone().unwrap_or_else(default_flash_bin),
        stock_bin: args.stock_bin.clone().unwrap_or_else(default_stock_bin),
        build_bin: args.build_bin.clone().unwrap_or_else(default_build_bin),
        wellfed_bin: args.wellfed_bin.clone().unwrap_or_else(default_wellfed_bin),
    };
    if args.ui {
        page::available()?;
    }
    // The full-screen console takes the console's lines from a channel;
    // without a terminal, they go to stdout as they come.
    let screen = tui::available(args.plain);
    let (console, lines) = if screen {
        let (send, lines) = std::sync::mpsc::sync_channel(CONSOLE_QUEUE);
        (Console::Lines(send), Some(lines))
    } else {
        (Console::Terminal, None)
    };
    let options = Options {
        root: root.to_path_buf(),
        tools: tools.clone(),
        console,
    };
    // The `wellfed` CLI finds the server through this, from any checkout or
    // key file; without it, it could open a session beside the server's.
    let rendezvous = rendezvous::directories();
    if rendezvous.is_empty() {
        anyhow::bail!(
            "Neither XDG_RUNTIME_DIR nor HOME is an absolute path: the `wellfed` CLI could not find this server"
        );
    }
    let device = DeviceOptions {
        address: args.feeder_address,
        clock: if args.no_clock_sync {
            ClockSync::Off
        } else {
            ClockSync::On
        },
        rendezvous,
        ..DeviceOptions::default()
    };
    let server = Server::open_with(options, &device, |config| open_board(args, config))?;

    let stop_page = Cancel::new();
    let page = if args.ui {
        let page = page::Page {
            root,
            flash_bin: &tools.flash_bin,
            address: &args.listen,
            port: args.ui_port,
        };
        Some(page.start(
            &stop_page,
            server.store(),
            server.outputs(),
            server.answers(),
        )?)
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
        "Bench server on {}, holding the bench lease; relays {} ({} stops it)",
        server.socket().display(),
        relays.join(", "),
        if screen { "q or Ctrl+C" } else { "Ctrl+C" }
    ));
    let (asks, asked) = tokio::sync::mpsc::unbounded_channel();
    let console = match lines {
        Some(lines) => Some(tui::Tui::start(server.store(), lines, asks.clone())?),
        None => None,
    };
    let stopper = server.stopper();
    let served = runtime.block_on(async {
        signals(asks)?;
        server
            .run(stopper.until_stop(asked))
            .await
            .map_err(anyhow::Error::from)
    });

    if let Some(console) = console {
        console.finish()?;
    }
    stop_page.cancel();
    if let Some(page) = page {
        let _ = page.join();
    }
    served?;
    Ok(0)
}

/// The configured relay board, or with `--fake-board` a scripted one that
/// starts with every relay off.
#[cfg_attr(
    not(feature = "fake-board"),
    expect(
        unused_variables,
        reason = "only a binary built with `fake-board` reads its flag"
    )
)]
fn open_board(args: &ServeArgs, config: &Config) -> wellfed_relay::Result<Box<dyn Board>> {
    #[cfg(feature = "fake-board")]
    if args.fake_board {
        let fake = wellfed_relay::FakeBoard::default();
        return Ok(Box::new(AtBoard::new(Box::new(fake))));
    }
    Ok(Box::new(AtBoard::open(&config.relay_port)?))
}

/// The operator's Ctrl+C, a SIGTERM, or the terminal hanging up, each as
/// an ask of the server's stopper, which stops through the job's cleanup
/// or, during a write, waits. The job's children run in groups of their
/// own, so a hangup reaches the server only, never a `wellfed-flash`
/// mid-write. The task keeps `asks` open for the server's life.
fn signals(asks: tokio::sync::mpsc::UnboundedSender<Ask>) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            let ask = tokio::select! {
                _ = interrupt.recv() => Ask::Interrupt,
                _ = terminate.recv() => Ask::Terminate,
                _ = hangup.recv() => Ask::Terminate,
            };
            if asks.send(ask).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// How long `status` waits for a server that may be stuck.
const STATUS_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// `status`: the lease first, then the server's view, or with no server the
/// relay board read directly.
fn status(root: &Path, break_lease: bool, runtime: &tokio::runtime::Runtime) -> anyhow::Result<u8> {
    report_lease(root, break_lease)?;

    let client = Client::new(root);
    let asked = runtime.block_on(async { tokio::time::timeout(STATUS_WAIT, client.state()).await });
    match asked {
        Ok(Ok(state)) => {
            report_state(&state);
            report_ports(root);
            Ok(0)
        }
        Ok(Err(ClientError::NoServer { .. })) => {
            wellfed_rig::say!("Server: none (`wellfed-bench serve` starts one)");
            Ok(direct_status(root)?)
        }
        Ok(Err(error)) => Err(error.into()),
        Err(_) => {
            wellfed_rig::say!(
                "Server: not answering on {} within {} s; the lease above says who holds the bench",
                root.join(wellfed_bench_server::api::SOCKET).display(),
                STATUS_WAIT.as_secs()
            );
            Ok(1)
        }
    }
}

/// Who holds the bench. With `--break-lease`, a lease whose holder is gone
/// is removed first; anything else refuses. Printed before the relay board
/// opens, so it shows even while a holder keeps the board's port.
fn report_lease(root: &Path, break_lease: bool) -> anyhow::Result<()> {
    if break_lease {
        // A killed server's flash client may still be writing: a new server
        // on a freed lease could open the UART under it. A live holder's
        // children are its own, and the break refuses that lease anyway.
        if let Ok(Some((_, Liveness::Gone))) = lease::inspect(root) {
            orphans::no_flash(root)?;
        }
        let holder = lease::break_stale(root)?;
        wellfed_rig::say!("Lease: broke the stale lease of {holder}");
    }

    match lease::inspect(root) {
        Ok(None) => wellfed_rig::say!("Lease: free"),
        Ok(Some((holder, Liveness::Alive))) => wellfed_rig::say!("Lease: held by {holder}"),
        Ok(Some((holder, Liveness::Unknown))) => {
            wellfed_rig::say!("Lease: held by {holder}, on another host");
        }
        Ok(Some((holder, Liveness::Gone))) => {
            wellfed_rig::say!(
                "Lease: left by {holder}, which is gone (a killed run?); check the relays below, \
                 then `status --break-lease` frees the bench"
            );
            for orphan in orphans::running(root) {
                wellfed_rig::say!(
                    "  still running here: pid {} `{}`",
                    orphan.pid,
                    orphan.command
                );
            }
        }
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
    if let Some(prompt) = &state.prompt {
        wellfed_rig::say!(
            "Job {} asks ({}): {}\n  `wellfed-bench answer {} {} {}` answers it",
            prompt.job,
            prompt.seq,
            prompt.text,
            prompt.job,
            prompt.seq,
            prompt.answer_words()
        );
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
        Uart::Writing { done, total, .. } => wellfed_rig::say!(
            "UART: WRITING FLASH ({done}/{total} sectors verified); do not touch the bench"
        ),
    }
    report_feeder(state);
    for (channel, relay) in &state.relays {
        wellfed_rig::say!("Relay {channel}: {}", on_off(relay.on));
    }
}

fn report_feeder(state: &BenchState) {
    let session = match &state.feeder.session {
        Session::Idle => "not tried yet".to_string(),
        Session::Unconfigured { error } => format!("unconfigured ({error})"),
        Session::Paused { pause } => format!("paused: {pause}"),
        Session::Connecting { attempt, error, .. } => format!(
            "connecting, attempt {}{}",
            attempt + 1,
            error
                .as_deref()
                .map(|e| format!(" (last: {e})"))
                .unwrap_or_default()
        ),
        Session::Open { address, build, .. } => {
            format!("open at {address}, build {}", short(build))
        }
    };
    wellfed_rig::say!("Feeder session: {session}");
    if state.feeder.other_sessions > 0 {
        wellfed_rig::say!(
            "  WARNING: {} other device API session(s) open with the feeder, besides the server's",
            state.feeder.other_sessions
        );
    }
    if let Some(status) = &state.feeder.status {
        let clock = match state.feeder.clock.skew_ms {
            Some(skew) => format!("clock {skew:+} ms"),
            None => "clock not set".into(),
        };
        wellfed_rig::say!(
            "  status: build {} on bank {}, up {} s, boot {}, {clock}",
            short(&status.build),
            status.bank,
            status.uptime_ms / 1000,
            status.boot_count
        );
    }
    if let Some(start) = &state.capture.start {
        wellfed_rig::say!(
            "  UART: last start build {} on bank {}",
            short(&start.build),
            start
                .bank
                .map_or_else(|| "?".into(), |bank| bank.to_string())
        );
    }
    for (bank, entry) in &state.banks.banks {
        wellfed_rig::say!("Bank {}: {}", bank_number(*bank), entry_text(entry));
    }
}

fn short(build: &str) -> &str {
    build.get(..12).unwrap_or(build)
}

pub(crate) fn bank_number(bank: Bank) -> u8 {
    match bank {
        Bank::Zero => 0,
        Bank::One => 1,
    }
}

fn content_text(content: &Content) -> String {
    match content {
        Content::Build { id } => format!("build {}", short(id)),
        Content::Stock => "stock".into(),
        Content::Invalid => "no valid image".into(),
        Content::Image { sha256 } => format!("the image {}", short(sha256)),
        Content::Unknown => "unknown (a write stopped partway)".into(),
    }
}

/// One ledger entry as a line: what, from which job, and the feeder's word.
pub(crate) fn entry_text(entry: &Entry) -> String {
    let check = match &entry.check {
        Check::Unchecked => "unchecked".to_string(),
        Check::Confirmed { by, .. } => format!("confirmed by the {by:?}").to_lowercase(),
        Check::Disputed { by, seen, .. } => format!(
            "DISPUTED: the {} reports {}",
            format!("{by:?}").to_lowercase(),
            content_text(seen)
        ),
    };
    format!(
        "{} ({:?}{}), {check}",
        content_text(&entry.content),
        entry.source,
        entry
            .session
            .as_deref()
            .map(|session| format!(" {session}"))
            .unwrap_or_default()
    )
}

/// `banks`: the ledger, or with `--seed` its starting point first.
async fn banks(client: &Client, args: BanksArgs) -> anyhow::Result<u8> {
    let ledger = match args.seed {
        Some(session) => {
            let seeded = client
                .seed(&Seed {
                    session,
                    bank: args.bank,
                    build: args.build,
                })
                .await?;
            let from = match &seeded.from {
                SeededFrom::Inspection { inspection } => format!("from {inspection}"),
                SeededFrom::Operator { inspection } => {
                    format!("from --bank and --build ({inspection} names no build)")
                }
            };
            wellfed_rig::say!(
                "Seeded bank {} with build {} {from}",
                seeded.bank,
                short(&seeded.build)
            );
            seeded.ledger
        }
        None => client.banks().await?,
    };
    if ledger.banks.is_empty() {
        wellfed_rig::say!("The ledger is empty; `banks --seed DUMP_SESSION` starts it");
    }
    for (bank, entry) in &ledger.banks {
        wellfed_rig::say!("Bank {}: {}", bank_number(*bank), entry_text(entry));
    }
    Ok(0)
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
        written: Vec::new(),
    };
    bench.status_report()
}

#[cfg(test)]
mod tests;
