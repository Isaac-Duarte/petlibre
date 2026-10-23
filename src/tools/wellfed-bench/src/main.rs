//! `wellfed-bench`: relay power, the loader strap, and supervised
//! captures, dumps and updates through `wellfed-flash`. `--ui` serves the
//! read-only progress page, alone or beside a command.
//!
//! Every subcommand except `status` is a physical bench operation. All but
//! `status` record their steps under `private/bench-status`, which the page
//! reads; `demo` refuses to start while another command is recorded as
//! running there.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use clap::{Parser, Subcommand};

use wellfed_bench::commands::{
    self, Bench, DemoArgs, ExecuteArgs, GiveWayArgs, GiveWayExecuteArgs, ItestArgs, OtaArgs,
    StockArgs, StockExecuteArgs, StockOtaArgs, TransientArgs, UpdateArgs, check_name, pattern_name,
};
use wellfed_bench::config::{Config, STATUS_DIR};
use wellfed_bench::relay::Relay;
use wellfed_bench::status::{self, Status};
use wellfed_bench::tools::{Processes, default_build_bin, default_flash_bin, default_local_bin};
use wellfed_bench::{Error, INTERRUPTED};

#[derive(Parser)]
#[command(
    version,
    about = "Relay bench for the PLAF103 research unit",
    long_about = "Relay bench for the PLAF103 research unit.\n\n\
        Relay 1 switches the feeder's USB 5V; relay 2 holds feeder TX to GND through the 4.7 kΩ strap. \
        Serial work is delegated unchanged to wellfed-flash, so its pinned checks, plan hashes and \
        no-retry rules apply. A failed step stops without retry; a stopped update is never followed \
        by a boot. Ports and evidence paths come from the ignored private/bench.json; see \
        docs/hardware/bench.md. Running a command is a physical bench operation.",
    arg_required_else_help = true
)]
struct Cli {
    /// Repository checkout whose ignored private/ holds all evidence.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    /// Power-off time before a cold boot, in seconds.
    #[arg(long, global = true, default_value_t = 2.0)]
    off_seconds: f64,
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
    /// Repeated logic-analyzer captures of power-ons or loader entries (FW30).
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
    /// server's health; never touches power.
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

    /// Click the spare relays 3 and 4 in patterns; never switches power or the strap.
    Demo(DemoArgs),
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
            Self::Demo(_) => "demo",
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Status | Self::Off | Self::On | Self::Demo(_) => None,
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
}

fn main() -> ExitCode {
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

fn run(cli: Cli) -> wellfed_bench::Result<u8> {
    let root = cli
        .root
        .canonicalize()
        .map_err(|error| Error::Bench(format!("--root {}: {error}", cli.root.display())))?;
    let tools = Processes {
        root: root.clone(),
        flash_bin: cli.flash_bin.unwrap_or_else(default_flash_bin),
        local_bin: cli.local_bin.unwrap_or_else(default_local_bin),
        build_bin: cli.build_bin.unwrap_or_else(default_build_bin),
    };

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
    if let Command::Itest(args) = &command
        && args.list
    {
        for line in commands::itest::listing() {
            wellfed_bench::say!("{line}");
        }
        return Ok(0);
    }
    if let Some(name) = command.name() {
        check_name(name)?;
    }

    // The demo refuses to click relays beside another bench command.
    let status_dir = root.join(STATUS_DIR);
    if matches!(command, Command::Demo(_))
        && let Some(running) = status::busy(&status_dir)
    {
        return Err(Error::Bench(format!(
            "Bench command '{running}' is running; try again when it ends"
        )));
    }

    let local_bin = tools.local_bin.clone();
    let mut bench = open_bench(root, tools, &command, cli.off_seconds)?;
    // `status` only reads, so it leaves no status record behind.
    if !matches!(command, Command::Status) {
        record_start(&mut bench, &command, status_dir)?;
    }

    let result = dispatch(&mut bench, &command, local_bin);
    let (code, error) = match &result {
        Ok(code) => (*code, None),
        Err(error) => (1, Some(error.to_string())),
    };
    bench.status.finish(code, error);
    result
}

/// Bind the progress page, so a busy port refuses before any bench command.
#[cfg(feature = "ui")]
fn start_page(
    root: &Path,
    flash_bin: &Path,
    listen: &str,
    port: u16,
) -> wellfed_bench::Result<std::thread::JoinHandle<()>> {
    let listen = wellfed_bench::ui::Listen::new(listen, port)?;
    let handle = wellfed_bench::ui::start(root.to_path_buf(), flash_bin.to_path_buf(), &listen)?;
    wellfed_bench::say!("Bench page on {} (Ctrl+C to stop)", listen.url());
    Ok(handle)
}

/// Without the web app built in, `--ui` refuses before any bench command.
#[cfg(not(feature = "ui"))]
fn start_page(
    _root: &Path,
    _flash_bin: &Path,
    _listen: &str,
    _port: u16,
) -> wellfed_bench::Result<std::thread::JoinHandle<()>> {
    Err(Error::Bench(
        "This wellfed-bench was built without the web page; run `make ui`, then \
         build with `--features ui`"
            .into(),
    ))
}

/// The bench a command runs on: its config, the relay board when the
/// command may switch it, and the real tools.
fn open_bench(
    root: PathBuf,
    tools: Processes,
    command: &Command,
    off_seconds: f64,
) -> wellfed_bench::Result<Bench> {
    let config = Config::load(&root)?;
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
        sleep: wellfed_bench::sleep,
        off_seconds,
    })
}

/// Start the command's progress record for the page: its steps and the
/// flash client it runs.
fn record_start(
    bench: &mut Bench,
    command: &Command,
    status_dir: PathBuf,
) -> wellfed_bench::Result<()> {
    let label = command.label();
    let steps: Vec<&str> = match command {
        Command::Demo(args) => commands::demo_patterns(args)
            .into_iter()
            .map(pattern_name)
            .collect(),
        Command::Capture { .. } => vec!["capture"],
        Command::Ota(args) => commands::ota::steps(args.source()),
        Command::Itest(args) => commands::itest::steps(args)?,
        Command::StockOta(args) => args.steps(),
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
    if matches!(command, Command::Itest(_) | Command::StockOta(_)) {
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
) -> wellfed_bench::Result<u8> {
    let captures = bench.root.join(wellfed_bench::config::CAPTURES);
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
        Command::Itest(args) => bench.itest(args, local_bin),
        Command::StockOta(args) => bench.stock_ota(args, local_bin),
        Command::Demo(args) => bench.demo(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ota_takes_its_flags_and_the_global_local_bin() {
        let cli = Cli::try_parse_from([
            "wellfed-bench",
            "ota",
            "fw12-x",
            "--review",
            "private/verification/r",
            "--version",
            "14",
            "--minutes",
            "5",
            "--local-bin",
            "bin/wellfed-local",
        ])
        .unwrap();

        let Some(Command::Ota(args)) = &cli.command else {
            panic!("not ota")
        };
        assert_eq!((args.version, args.minutes), (Some(14), 5.0));
        assert!(!args.attended_motion && !args.build);
        assert_eq!(args.config, PathBuf::from("private/server.json"));
        assert_eq!(cli.local_bin, Some(PathBuf::from("bin/wellfed-local")));
    }

    #[test]
    fn stock_ota_takes_a_name_its_firmware_and_no_promote() {
        let cli = Cli::try_parse_from([
            "wellfed-bench",
            "stock-ota",
            "poc-01",
            "--firmware",
            "ble",
            "--no-promote",
        ])
        .unwrap();
        let Some(Command::StockOta(args)) = &cli.command else {
            panic!("not stock-ota")
        };
        assert_eq!(args.name, "poc-01");
        assert!(args.no_promote && !args.power);
        assert_eq!(cli.command.as_ref().map(Command::label), Some("stock-ota"));

        assert!(Cli::try_parse_from(["wellfed-bench", "stock-ota"]).is_err());
    }

    #[test]
    fn itest_lists_without_a_name_and_takes_its_scenarios() {
        let cli = Cli::try_parse_from(["wellfed-bench", "itest", "--list"]).unwrap();
        let Some(Command::Itest(args)) = &cli.command else {
            panic!("not itest")
        };
        assert!(args.list && args.name.is_none());

        let cli = Cli::try_parse_from([
            "wellfed-bench",
            "itest",
            "run1",
            "--build",
            "--power",
            "--only",
            "requests,network-bounce",
        ])
        .unwrap();
        let Some(Command::Itest(args)) = &cli.command else {
            panic!("not itest")
        };
        assert_eq!(args.only, ["requests", "network-bounce"]);
        assert!(args.build && args.power && !args.keep_going);
        assert_eq!(cli.command.as_ref().unwrap().name(), Some("run1"));

        for argv in [
            &["wellfed-bench", "itest"][..],
            &[
                "wellfed-bench",
                "itest",
                "run1",
                "--build",
                "--review",
                "r",
            ],
        ] {
            assert!(Cli::try_parse_from(argv).is_err(), "{argv:?}");
        }
    }

    fn ota(extra: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(["wellfed-bench", "ota", "fw12-x"].iter().chain(extra))
    }

    #[test]
    fn ota_build_takes_a_no_motion_image_and_config() {
        let cli = ota(&[
            "--build",
            "--firmware",
            "trial",
            "--variant",
            "fw11-fail",
            "--config",
            "private/other.json",
        ])
        .unwrap();

        let Some(Command::Ota(args)) = &cli.command else {
            panic!("not ota")
        };
        assert!(args.build && args.review.is_none());
        assert_eq!(args.firmware, Some(commands::Firmware::Trial));
        assert_eq!(args.variant, Some(commands::Variant::Fw11Fail));
        assert_eq!(args.config, PathBuf::from("private/other.json"));
    }

    #[test]
    fn ota_refuses_motion_and_mixed_sources() {
        for motion in [&["--variant", "fw16-pulse"], &["--firmware", "motor"]] {
            let args = [&["--build"][..], motion].concat();
            let error = ota(&args).err().unwrap().to_string();
            assert!(error.contains("motion builds stay manual"), "{error}");
        }

        // --firmware and --variant without --build are refused by the
        // command.
        for extra in [&["--build", "--review", "private/verification/r"][..], &[]] {
            assert!(ota(extra).is_err(), "{extra:?}");
        }
    }
}
