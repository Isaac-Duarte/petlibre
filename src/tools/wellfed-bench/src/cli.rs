//! `wellfed-bench`'s command line: relay power, the loader strap, and
//! supervised captures, dumps and updates through `wellfed-flash`. `--ui`
//! serves the read-only progress page (`wellfed-dashboard`), alone or beside
//! a command.
//!
//! Every subcommand except `status` is a physical bench operation. All but
//! `status` record their steps under `private/bench-status`, which the page
//! reads.
//!
//! Two binaries share this command line. `wellfed-bench` runs everything but
//! the suite's commands (`itest`, `stock-ota`), which it hands, with the
//! arguments it was given, to `wellfed-itest`: the suite links this crate,
//! so this crate's binary cannot link the suite. `wellfed-itest` runs only
//! those two.

use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use clap::{Parser, Subcommand};
use wellfed_formats::bench::catalog;
use wellfed_formats::bench::progress::Status;
use wellfed_formats::bench::{CONFIG, Config, STATUS_DIR};
use wellfed_relay::Relay;

use crate::commands::{
    self, Bench, ExecuteArgs, GiveWayArgs, GiveWayExecuteArgs, OtaArgs, StockArgs,
    StockExecuteArgs, TransientArgs, UartWhenOff, UpdateArgs, check_name,
};
use crate::suite::{ItestArgs, StockOtaArgs, Suite};
use crate::tools::{
    Processes, default_build_bin, default_flash_bin, default_itest_bin, default_local_bin,
};
use crate::{Error, INTERRUPTED};

#[derive(Parser)]
#[command(
    version,
    about = "Relay bench for the PLAF103 research unit",
    long_about = "Relay bench for the PLAF103 research unit.\n\n\
        Relay 1 switches the feeder's USB 5V; relay 2 holds feeder TX to GND through the 4.7 kΩ strap. \
        Serial work is delegated unchanged to wellfed-flash, so its pinned checks, plan hashes and \
        no-retry rules apply. A failed step stops without retry; a stopped update is never followed \
        by a boot. Ports and evidence paths come from the ignored private/bench.json; see \
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
    /// The wellfed-flash binary (default: beside this one).
    #[arg(long, global = true)]
    flash_bin: Option<PathBuf>,
    /// The wellfed-local binary for `ota` and `itest` (default: beside this one).
    #[arg(long, global = true)]
    local_bin: Option<PathBuf>,
    /// The wellfed-build binary for `--build` (default: beside this one).
    #[arg(long, global = true)]
    build_bin: Option<PathBuf>,

    /// Serve the read-only progress page: alone until Ctrl+C, or while a command runs.
    #[arg(long, global = true)]
    ui: bool,
    /// Address for the page: loopback, or one named interface such as Tailscale.
    #[arg(long, global = true, default_value = "127.0.0.1", requires = "ui")]
    listen: String,
    #[arg(long, global = true, default_value_t = 8780, requires = "ui")]
    ui_port: u16,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show relay states and port presence.
    Status,
    /// Release the strap and cut feeder power.
    Off,
    /// Power the feeder without the strap or a capture.
    On,

    /// Record the running feeder without touching power.
    Capture {
        /// Capture name under private/captures/.
        name: String,
        #[arg(long, default_value_t = 60.0)]
        seconds: f64,
    },
    /// Cold boot into the application while capturing.
    Boot {
        /// Capture name under private/captures/.
        name: String,
        #[arg(long, default_value_t = 30.0)]
        seconds: f64,
    },
    /// Cold boot into the ROM loader and leave it powered.
    Loader {
        /// Capture name under private/captures/.
        name: String,
    },
    /// Repeated logic-analyzer captures of power-ons or loader entries.
    Transients(TransientArgs),

    /// Loader, bounded read, then full flash read.
    Dump {
        /// Session name under private/flash-backups/.
        name: String,
    },
    /// Loader, bounded read, inspect and plan; stops for review.
    FlashPrepare(UpdateArgs),
    /// Execute the reviewed plan, then boot.
    FlashExecute(ExecuteArgs),
    /// Loader, bounded read, give-way inspection and plan; stops for review.
    GiveWayPrepare(GiveWayArgs),
    /// Clear bank 1's first marker byte, then boot.
    GiveWayExecute(GiveWayExecuteArgs),
    /// The pinned original stock back into bank 1 over UART: with --fallback,
    /// the give-way to bank 0 first; otherwise loader, full read and the
    /// recovery plan. Stops for review.
    StockPrepare(StockArgs),
    /// Execute what stock-prepare planned, then boot.
    StockExecute(StockExecuteArgs),
    /// Sign and offer a review (or `--build` one first) to the running local
    /// server, watch the UART until it confirms or falls back, then check the
    /// feeder's status over the device API; never touches power.
    Ota(OtaArgs),
    /// Real-device integration scenarios: runs its own hotspot, server and
    /// UART capture, installs over the air, restores the bench afterwards.
    /// Cold boots only with --power; never flashes over UART.
    Itest(ItestArgs),
    /// Stock-to-replacement POC: with the feeder on stock 1.1.15, build a
    /// replacement image and have stock install it over its own updater.
    /// Runs its own hotspot, trial-enabled server and UART capture, sends
    /// stock one request, restores the bench afterwards. Never flashes over
    /// UART and never changes routing (`wellfed-local network apply` is the
    /// operator's).
    StockOta(StockOtaArgs),
}

impl Command {
    fn label(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Off => "off",
            Self::On => "on",
            Self::Capture { .. } => "capture",
            Self::Boot { .. } => "boot",
            Self::Loader { .. } => "loader",
            Self::Transients(_) => "transients",
            Self::Dump { .. } => "dump",
            Self::FlashPrepare(_) => "flash-prepare",
            Self::FlashExecute(_) => "flash-execute",
            Self::GiveWayPrepare(_) => "give-way-prepare",
            Self::GiveWayExecute(_) => "give-way-execute",
            Self::StockPrepare(_) => "stock-prepare",
            Self::StockExecute(_) => "stock-execute",
            Self::Ota(_) => "ota",
            Self::Itest(_) => "itest",
            Self::StockOta(_) => "stock-ota",
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Status | Self::Off | Self::On => None,
            Self::Capture { name, .. }
            | Self::Boot { name, .. }
            | Self::Loader { name }
            | Self::Dump { name } => Some(name),
            Self::Transients(args) => Some(&args.name),
            Self::FlashPrepare(args) => Some(&args.name),
            Self::FlashExecute(args) => Some(&args.update.name),
            Self::GiveWayPrepare(args) => Some(&args.name),
            Self::GiveWayExecute(args) => Some(&args.give_way.name),
            Self::StockPrepare(args) => Some(&args.name),
            Self::StockExecute(args) => Some(&args.stock.name),
            Self::Ota(args) => Some(&args.name),
            Self::Itest(args) => args.name.as_deref(),
            Self::StockOta(args) => Some(&args.name),
        }
    }

    /// One of the suite's commands, which `wellfed-itest` runs.
    fn suite(&self) -> bool {
        matches!(self, Self::Itest(_) | Self::StockOta(_))
    }
}

/// Parse the command line and run it. With `suite` linked in (the
/// `wellfed-itest` binary), only the suite's commands run; without it
/// (`wellfed-bench`), those are handed to `wellfed-itest`.
pub fn main(suite: Option<&dyn Suite>) -> ExitCode {
    let cli = Cli::parse();

    // An interrupt stops the running sequence, through its power-off cleanup.
    // wellfed-flash children get the terminal's interrupt too, and stop themselves.
    let _ = ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst));
    // Evidence and status are private.
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));

    match run(cli, suite) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Stopped: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli, suite: Option<&dyn Suite>) -> crate::Result<u8> {
    let root = cli
        .root
        .canonicalize()
        .map_err(|error| Error::Bench(format!("--root {}: {error}", cli.root.display())))?;
    let tools = Processes::new(
        root.clone(),
        cli.flash_bin.unwrap_or_else(default_flash_bin),
        cli.local_bin.unwrap_or_else(default_local_bin),
        cli.build_bin.unwrap_or_else(default_build_bin),
    );

    let itest_bin = default_itest_bin();
    let handed = std::env::var(FINGERPRINT_VAR).ok();
    let binary = if suite.is_some() {
        Binary::Itest {
            handed: handed.as_deref(),
        }
    } else {
        Binary::Bench {
            itest_bin: &itest_bin,
            itest_built: itest_bin.is_file(),
        }
    };
    match route(binary, cli.command.as_ref(), crate::BUILD_FINGERPRINT) {
        Route::List => {
            for line in catalog::listing() {
                crate::say!("{line}");
            }
            return Ok(0);
        }
        Route::HandOff => return Err(hand_to_suite(&itest_bin)),
        Route::Refuse(refusal) => return Err(refusal.into()),
        Route::Run => {}
    }

    let page = if cli.ui {
        Some(start_page(
            suite.is_some(),
            &root,
            &tools.flash_bin,
            &cli.listen,
            cli.ui_port,
        )?)
    } else {
        None
    };

    let Some(command) = cli.command else {
        // Viewer only: serve until the operator interrupts.
        if let Some(page) = page {
            let _ = page.join();
        }
        return Ok(0);
    };
    if let Some(name) = command.name() {
        check_name(name)?;
    }

    let status_dir = root.join(STATUS_DIR);

    let local_bin = tools.local_bin.clone();
    let mut bench = open_bench(root, tools, &command, cli.off_seconds, cli.uart_when_off)?;
    // `status` only reads, so it leaves no status record behind.
    if !matches!(command, Command::Status) {
        record_start(&mut bench, &command, status_dir, suite)?;
    }

    let result = dispatch(&mut bench, &command, local_bin, suite);
    let (code, error) = match &result {
        Ok(code) => (*code, None),
        Err(error) => (1, Some(error.to_string())),
    };
    bench.status.finish(code, error);
    result
}

/// The variable the bench sets at a hand-off to its [`crate::BUILD_FINGERPRINT`].
const FINGERPRINT_VAR: &str = "WELLFED_BENCH_FINGERPRINT";

/// Both binaries' builds, as a refusal tells the operator to make them.
const REBUILD: &str = "cargo build --locked --release -p wellfed-bench -p wellfed-itest \
     (with --features ui for --ui)";

/// Which binary parsed the command line, and what it knows of the other.
#[derive(Debug, Clone, Copy)]
enum Binary<'a> {
    /// `wellfed-bench`, and the `wellfed-itest` it would hand the suite to.
    Bench {
        itest_bin: &'a Path,
        itest_built: bool,
    },
    /// `wellfed-itest`, with the fingerprint a handing bench passed, if any.
    Itest { handed: Option<&'a str> },
}

/// What a binary does with its command line.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// `itest --list`, which needs no bench.
    List,
    /// Replace this process with `wellfed-itest`.
    HandOff,
    Refuse(Refusal),
    Run,
}

/// Why a binary will not run a command line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("{0} not found; build both binaries with {REBUILD}")]
    ItestMissing(PathBuf),
    #[error(
        "wellfed-itest runs only itest and stock-ota; the bench's other commands and the page \
         alone are wellfed-bench's"
    )]
    NotSuite,
    /// The handing bench was built from other sources: this suite would run
    /// another version of the relay and cleanup code than the bench's.
    #[error(
        "wellfed-itest was built from other sources than the wellfed-bench that handed it \
         this command; rebuild both with {REBUILD}"
    )]
    Stale,
}

/// Decide, from the parsed command line alone, whether this binary runs it,
/// hands it to `wellfed-itest`, or refuses it. `own` is this build's
/// fingerprint.
fn route(binary: Binary<'_>, command: Option<&Command>, own: &str) -> Route {
    if let Some(Command::Itest(args)) = command
        && args.list
    {
        return Route::List;
    }
    let suite = command.is_some_and(Command::suite);

    match binary {
        Binary::Bench { .. } if !suite => Route::Run,
        Binary::Bench {
            itest_bin,
            itest_built: false,
        } => Route::Refuse(Refusal::ItestMissing(itest_bin.to_path_buf())),
        Binary::Bench { .. } => Route::HandOff,
        Binary::Itest { .. } if !suite => Route::Refuse(Refusal::NotSuite),
        Binary::Itest {
            handed: Some(handed),
        } if handed != own => Route::Refuse(Refusal::Stale),
        Binary::Itest { .. } => Route::Run,
    }
}

/// Replace this process with `wellfed-itest`, given the same arguments and
/// this build's fingerprint, so the suite's run, its interrupt and its exit
/// status are the command's own. Returns only if that fails.
fn hand_to_suite(itest_bin: &Path) -> Error {
    let mut args = std::env::args_os();
    let name = args
        .next()
        .unwrap_or_else(|| OsString::from("wellfed-bench"));
    let error = std::process::Command::new(itest_bin)
        .arg0(name)
        .args(args)
        .env(FINGERPRINT_VAR, crate::BUILD_FINGERPRINT)
        .exec();
    Error::Bench(format!("{}: {error}", itest_bin.display()))
}

/// Bind the progress page, so a busy port refuses before any bench command.
#[cfg(feature = "ui")]
fn start_page(
    _suite: bool,
    root: &Path,
    flash_bin: &Path,
    listen: &str,
    port: u16,
) -> crate::Result<std::thread::JoinHandle<()>> {
    let listen = wellfed_dashboard::Listen::new(listen, port)?;
    let handle = wellfed_dashboard::start(
        root.to_path_buf(),
        flash_bin.to_path_buf(),
        &listen,
        &INTERRUPTED,
    )?;
    crate::say!("Bench page on {} (Ctrl+C to stop)", listen.url());
    Ok(handle)
}

/// Without the web app built in, `--ui` refuses before any bench command.
/// `suite`: this is `wellfed-itest`, which a bench built with the page may
/// have handed `--ui`, so the refusal names it.
#[cfg(not(feature = "ui"))]
fn start_page(
    suite: bool,
    _root: &Path,
    _flash_bin: &Path,
    _listen: &str,
    _port: u16,
) -> crate::Result<std::thread::JoinHandle<()>> {
    let binary = if suite {
        "wellfed-itest"
    } else {
        "wellfed-bench"
    };
    Err(Error::Bench(format!(
        "{binary} was built without the web page; run `make ui`, then build it with \
         `--features ui`: {REBUILD}"
    )))
}

/// The bench a command runs on: its config, the relay board when the
/// command may switch it, and the real tools.
fn open_bench(
    root: PathBuf,
    tools: Processes,
    command: &Command,
    off_seconds: f64,
    uart_when_off: UartWhenOff,
) -> crate::Result<Bench> {
    let config: Config = crate::load(&root, Path::new(CONFIG))?;
    // The over-the-air loop only captures, so it leaves the relay board free;
    // the integration suite holds it only for --power's cold boots.
    let relay = match command {
        Command::Ota(_) => Relay::detached(),
        Command::Itest(args) if !args.power => Relay::detached(),
        Command::StockOta(args) if !args.power => Relay::detached(),
        _ => Relay::open(&config.relay_port)?,
    };

    Ok(Bench {
        root,
        config,
        relay,
        status: Status::disabled(),
        tools: Box::new(tools),
        sleep: crate::sleep,
        off_seconds,
        uart_when_off,
    })
}

/// Start the command's progress record for the page: its steps and the
/// flash client it runs.
fn record_start(
    bench: &mut Bench,
    command: &Command,
    status_dir: PathBuf,
    suite: Option<&dyn Suite>,
) -> crate::Result<()> {
    let label = command.label();
    let steps: Vec<&str> = match command {
        Command::Capture { .. } => vec!["capture"],
        Command::Ota(args) => commands::ota::steps(args.source()),
        Command::Itest(args) => suite_of(suite)?.itest_steps(args)?,
        Command::StockOta(args) => suite_of(suite)?.stock_ota_steps(args),
        Command::StockPrepare(args) if args.fallback.is_some() => {
            commands::steps("give-way-prepare").to_vec()
        }
        Command::StockExecute(args) if args.stock.fallback.is_some() => {
            commands::steps("give-way-execute").to_vec()
        }
        _ => commands::steps(label).to_vec(),
    };

    bench.status = Status::new(&bench.root, status_dir);
    bench.status.begin(label, command.name(), &steps)?;
    if command.suite() {
        bench.status.fix_steps();
    }
    if let Some(tool) = bench.tools.flash_tool() {
        bench.status.flash_tool(tool);
    }
    Ok(())
}

fn dispatch(
    bench: &mut Bench,
    command: &Command,
    local_bin: PathBuf,
    suite: Option<&dyn Suite>,
) -> crate::Result<u8> {
    let captures = bench.root.join(wellfed_formats::bench::CAPTURES);
    match command {
        Command::Status => bench.status_report(),
        Command::Off => bench.off(),
        Command::On => bench.on(),
        Command::Capture { name, seconds } => {
            bench.status.step("capture");
            bench.capture(name, *seconds)
        }
        Command::Boot { name, seconds } => bench.boot(&captures.join(name), *seconds).map(|_| 0),
        Command::Loader { name } => bench.loader(&captures.join(name)).map(|_| 0),
        Command::Transients(args) => bench.transients(args),
        Command::Dump { name } => bench.dump(name),
        Command::FlashPrepare(args) => bench.flash_prepare(args),
        Command::FlashExecute(args) => bench.flash_execute(args),
        Command::GiveWayPrepare(args) => bench.give_way_prepare(args),
        Command::GiveWayExecute(args) => bench.give_way_execute(args),
        Command::StockPrepare(args) => bench.stock_prepare(args),
        Command::StockExecute(args) => bench.stock_execute(args),
        Command::Ota(args) => bench.ota(args),
        Command::Itest(args) => suite_of(suite)?.itest(bench, args, local_bin),
        Command::StockOta(args) => suite_of(suite)?.stock_ota(bench, args, local_bin),
    }
}

/// The linked suite; `run` hands the suite's commands on before any bench
/// opens when there is none, so this refusal is never reached.
fn suite_of(suite: Option<&dyn Suite>) -> crate::Result<&dyn Suite> {
    suite.ok_or_else(|| Error::Bench("This binary does not link the integration suite".into()))
}

#[cfg(test)]
mod tests;
