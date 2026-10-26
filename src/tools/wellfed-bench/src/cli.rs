//! `wellfed-bench`'s command line: relay power, the loader strap, and
//! supervised captures, dumps and updates through `wellfed-flash`. `--ui`
//! serves the read-only progress page (`wellfed-dashboard`), alone or beside
//! a command.
//!
//! Every subcommand except `status` is a physical bench operation. All but
//! `status` record their steps under `private/bench-status`, which the page
//! reads.
//!
//! The rig's sequences (`wellfed-rig`) and the integration suite's commands
//! (`itest`, `stock-ota`, `wellfed-itest`) run in this one process, so a
//! run, its interrupt and its exit status are the command's own.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use clap::{Parser, Subcommand};
use wellfed_formats::bench::catalog;
use wellfed_formats::bench::progress::{BenchCommand, Status};
use wellfed_formats::bench::{CONFIG, Config, STATUS_DIR};
use wellfed_itest::{ItestArgs, StockOtaArgs};
use wellfed_relay::Relay;
use wellfed_rig::INTERRUPTED;
use wellfed_rig::commands::{
    self, Bench, ExecuteArgs, GiveWayArgs, GiveWayExecuteArgs, OtaArgs, StockArgs,
    StockExecuteArgs, TransientArgs, UartWhenOff, UpdateArgs, check_name,
};
use wellfed_rig::tools::{Processes, default_build_bin, default_flash_bin, default_stock_bin};

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
    /// The wellfed-stock binary for `itest` and `stock-ota` (default: beside this one).
    #[arg(long, global = true)]
    stock_bin: Option<PathBuf>,
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
    /// UART and never changes routing (`wellfed-stock network apply` is the
    /// operator's).
    StockOta(StockOtaArgs),
}

impl Command {
    fn label(&self) -> BenchCommand {
        match self {
            Self::Status => BenchCommand::Status,
            Self::Off => BenchCommand::Off,
            Self::On => BenchCommand::On,
            Self::Capture { .. } => BenchCommand::Capture,
            Self::Boot { .. } => BenchCommand::Boot,
            Self::Loader { .. } => BenchCommand::Loader,
            Self::Transients(_) => BenchCommand::Transients,
            Self::Dump { .. } => BenchCommand::Dump,
            Self::FlashPrepare(_) => BenchCommand::FlashPrepare,
            Self::FlashExecute(_) => BenchCommand::FlashExecute,
            Self::GiveWayPrepare(_) => BenchCommand::GiveWayPrepare,
            Self::GiveWayExecute(_) => BenchCommand::GiveWayExecute,
            Self::StockPrepare(_) => BenchCommand::StockPrepare,
            Self::StockExecute(_) => BenchCommand::StockExecute,
            Self::Ota(_) => BenchCommand::Ota,
            Self::Itest(_) => BenchCommand::Itest,
            Self::StockOta(_) => BenchCommand::StockOta,
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

    /// One of the integration suite's commands.
    fn suite(&self) -> bool {
        matches!(self, Self::Itest(_) | Self::StockOta(_))
    }
}

/// Parse the command line and run it.
pub(crate) fn main() -> ExitCode {
    let cli = Cli::parse();

    // An interrupt stops the running sequence, through its power-off cleanup.
    // wellfed-flash children get the terminal's interrupt too, and stop themselves.
    let _ = ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst));
    // Evidence and status are private.
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
    let tools = Processes::new(
        root.clone(),
        cli.flash_bin.unwrap_or_else(default_flash_bin),
        cli.stock_bin.unwrap_or_else(default_stock_bin),
        cli.build_bin.unwrap_or_else(default_build_bin),
    );

    // The catalog needs no bench.
    if let Some(Command::Itest(args)) = &cli.command
        && args.list
    {
        for line in catalog::listing() {
            wellfed_rig::say!("{line}");
        }
        return Ok(0);
    }

    let page = if cli.ui {
        Some(start_page(
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

    let stock_bin = tools.stock_bin.clone();
    let mut bench = open_bench(root, tools, &command, cli.off_seconds, cli.uart_when_off)?;
    // `status` only reads, so it leaves no status record behind.
    if !matches!(command, Command::Status) {
        record_start(&mut bench, &command, status_dir)?;
    }

    let result = dispatch(&mut bench, &command, stock_bin);
    let (code, error) = match &result {
        Ok(code) => (*code, None),
        Err(error) => (1, Some(error.to_string())),
    };
    bench.status.finish(code, error);
    Ok(result?)
}

/// Bind the progress page, so a busy port refuses before any bench command.
#[cfg(feature = "ui")]
fn start_page(
    root: &Path,
    flash_bin: &Path,
    listen: &str,
    port: u16,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    let listen = wellfed_dashboard::Listen::new(listen, port)?;
    let handle = wellfed_dashboard::start(
        root.to_path_buf(),
        flash_bin.to_path_buf(),
        &listen,
        &INTERRUPTED,
    )?;
    wellfed_rig::say!("Bench page on {} (Ctrl+C to stop)", listen.url());
    Ok(handle)
}

/// Without the web app built in, `--ui` refuses before any bench command.
#[cfg(not(feature = "ui"))]
fn start_page(
    _root: &Path,
    _flash_bin: &Path,
    _listen: &str,
    _port: u16,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    anyhow::bail!(
        "wellfed-bench was built without the web page; run `make ui`, then build it with \
         `cargo build --locked --release -p wellfed-bench --features ui`"
    )
}

/// The bench a command runs on: its config, the relay board when the
/// command may switch it, and the real tools.
fn open_bench(
    root: PathBuf,
    tools: Processes,
    command: &Command,
    off_seconds: f64,
    uart_when_off: UartWhenOff,
) -> wellfed_rig::Result<Bench> {
    let config: Config = wellfed_rig::load(&root, Path::new(CONFIG))?;
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
        sleep: wellfed_rig::sleep,
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
) -> wellfed_itest::Result<()> {
    let label = command.label();
    let steps: Vec<&str> = match command {
        Command::Capture { .. } => vec!["capture"],
        Command::Ota(args) if args.cancel => vec![commands::ota::CANCEL_STEP],
        Command::Ota(args) => commands::ota::steps(args.source()),
        Command::Itest(args) => wellfed_itest::itest_steps(args)?,
        Command::StockOta(args) => wellfed_itest::stock_ota_steps(args),
        Command::StockPrepare(args) if args.fallback.is_some() => {
            commands::steps(BenchCommand::GiveWayPrepare).to_vec()
        }
        Command::StockExecute(args) if args.stock.fallback.is_some() => {
            commands::steps(BenchCommand::GiveWayExecute).to_vec()
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

/// Run the command: the suite's on `wellfed-itest`, the rest on the rig.
fn dispatch(
    bench: &mut Bench,
    command: &Command,
    stock_bin: PathBuf,
) -> wellfed_itest::Result<u8> {
    let captures = bench.root.join(wellfed_formats::bench::CAPTURES);
    let ran = match command {
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
        Command::Itest(args) => return wellfed_itest::itest(bench, args, stock_bin),
        Command::StockOta(args) => return wellfed_itest::stock_ota(bench, args, stock_bin),
    };
    Ok(ran?)
}

#[cfg(test)]
mod tests;
