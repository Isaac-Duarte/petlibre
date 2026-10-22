//! `petlibre-bench`: relay power, the loader strap, and supervised
//! captures, dumps and updates through `petlibre-flash`. `--ui` serves the
//! read-only progress page, alone or beside a command.
//!
//! Every subcommand except `status` is a physical bench operation. All but
//! `status` record their steps under `private/bench-status`, which the page
//! reads; `demo` refuses to start while another command is recorded as
//! running there.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use clap::{Parser, Subcommand};

use petlibre_bench::commands::{
    self, Bench, DemoArgs, ExecuteArgs, GiveWayArgs, GiveWayExecuteArgs, ItestArgs, OtaArgs,
    TransientArgs, UpdateArgs, check_name, pattern_name,
};
use petlibre_bench::config::{Config, STATUS_DIR};
use petlibre_bench::relay::Relay;
use petlibre_bench::status::{self, Status};
use petlibre_bench::tools::{Processes, Tools, default_flash_bin, default_local_bin};
use petlibre_bench::{Error, INTERRUPTED, ui};

#[derive(Parser)]
#[command(
    version,
    about = "Relay bench for the PLAF103 research unit",
    long_about = "Relay bench for the PLAF103 research unit.\n\n\
        Relay 1 switches the feeder's USB 5V; relay 2 holds feeder TX to GND through the 4.7 kΩ strap. \
        Serial work is delegated unchanged to petlibre-flash, so its pinned checks, plan hashes and \
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
    /// The petlibre-flash binary (default: beside this one).
    #[arg(long, global = true)]
    flash_bin: Option<PathBuf>,
    /// The petlibre-local binary for `ota` and `itest` (default: beside this one).
    #[arg(long, global = true)]
    local_bin: Option<PathBuf>,

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
    /// Sign and offer a review (or `--build` one first) to the running local
    /// server, watch the UART until it confirms or falls back, then check the
    /// server's health; never touches power.
    Ota(OtaArgs),
    /// Real-device integration scenarios: runs its own hotspot, server and
    /// UART capture, installs over the air, restores the bench afterwards.
    /// Cold boots only with --power; never flashes over UART.
    Itest(ItestArgs),

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
            Self::Ota(_) => "ota",
            Self::Itest(_) => "itest",
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
            Self::Ota(args) => Some(&args.name),
            Self::Itest(args) => args.name.as_deref(),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // An interrupt stops the running sequence, through its power-off cleanup.
    // petlibre-flash children get the terminal's interrupt too, and stop themselves.
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

fn run(cli: Cli) -> petlibre_bench::Result<u8> {
    let root = cli
        .root
        .canonicalize()
        .map_err(|error| Error::Bench(format!("--root {}: {error}", cli.root.display())))?;
    let flash_bin = cli.flash_bin.clone().unwrap_or_else(default_flash_bin);

    let page = if cli.ui {
        let listen = ui::Listen::new(&cli.listen, cli.ui_port)?;
        let handle = ui::start(root.clone(), flash_bin.clone(), &listen)?;
        petlibre_bench::say!("Bench page on {} (Ctrl+C to stop)", listen.url());
        Some(handle)
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
            petlibre_bench::say!("{line}");
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

    let config = Config::load(&root)?;
    // The over-the-air loop only captures, so it leaves the relay board free;
    // the integration suite holds it only for --power's cold boots.
    let relay = match &command {
        Command::Ota(_) => Relay::detached(),
        Command::Itest(args) if !args.power => Relay::detached(),
        _ => Relay::open(&config.relay_port)?,
    };
    let local_bin = cli.local_bin.clone().unwrap_or_else(default_local_bin);
    let tools = Processes {
        root: root.clone(),
        flash_bin,
        local_bin: local_bin.clone(),
    };
    let flash_tool = tools.flash_tool();
    let mut bench = Bench {
        root: root.clone(),
        config,
        relay,
        status: Status::disabled(),
        tools: Box::new(tools),
        sleep: petlibre_bench::sleep,
        off_seconds: cli.off_seconds,
    };

    // `status` only reads, so it leaves no status record behind.
    let label = command.label();
    if !matches!(command, Command::Status) {
        bench.status = Status::new(&root, status_dir);
        let steps: Vec<&str> = match &command {
            Command::Demo(args) => commands::demo_patterns(args)
                .into_iter()
                .map(pattern_name)
                .collect(),
            Command::Capture { .. } => vec!["capture"],
            Command::Ota(args) => commands::ota::steps(args.build),
            Command::Itest(args) => commands::itest::select(args)?
                .iter()
                .map(|scenario| scenario.name)
                .collect(),
            _ => commands::steps(label).to_vec(),
        };
        bench.status.begin(label, command.name(), &steps)?;
        if matches!(command, Command::Itest(_)) {
            bench.status.fix_steps();
        }
        if let Some(tool) = flash_tool {
            bench.status.flash_tool(tool);
        }
    }

    let result = dispatch(&mut bench, &command, local_bin);
    let (code, error) = match &result {
        Ok(code) => (*code, None),
        Err(error) => (1, Some(error.to_string())),
    };
    bench.status.finish(code, error);
    result
}

fn dispatch(
    bench: &mut Bench,
    command: &Command,
    local_bin: PathBuf,
) -> petlibre_bench::Result<u8> {
    let captures = bench.root.join(petlibre_bench::config::CAPTURES);
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
        Command::Ota(args) => bench.ota(args),
        Command::Itest(args) => bench.itest(args, local_bin),
        Command::Demo(args) => bench.demo(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ota_takes_its_flags_and_the_global_local_bin() {
        let cli = Cli::try_parse_from([
            "petlibre-bench",
            "ota",
            "fw12-x",
            "--review",
            "private/verification/r",
            "--version",
            "14",
            "--minutes",
            "5",
            "--local-bin",
            "bin/petlibre-local",
        ])
        .unwrap();

        let Some(Command::Ota(args)) = &cli.command else {
            panic!("not ota")
        };
        assert_eq!((args.version, args.minutes), (Some(14), 5.0));
        assert!(!args.attended_motion && !args.build);
        assert_eq!(args.config, PathBuf::from("private/server.json"));
        assert_eq!(cli.local_bin, Some(PathBuf::from("bin/petlibre-local")));
    }

    #[test]
    fn itest_lists_without_a_name_and_takes_its_scenarios() {
        let cli = Cli::try_parse_from(["petlibre-bench", "itest", "--list"]).unwrap();
        let Some(Command::Itest(args)) = &cli.command else {
            panic!("not itest")
        };
        assert!(args.list && args.name.is_none());

        let cli = Cli::try_parse_from([
            "petlibre-bench",
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
            &["petlibre-bench", "itest"][..],
            &[
                "petlibre-bench",
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
        Cli::try_parse_from(["petlibre-bench", "ota", "fw12-x"].iter().chain(extra))
    }

    #[test]
    fn ota_build_takes_a_no_motion_variant_python_and_config() {
        let cli = ota(&[
            "--build",
            "--variant",
            "fw11-fail",
            "--python",
            "/usr/bin/python3",
            "--config",
            "private/other.json",
        ])
        .unwrap();

        let Some(Command::Ota(args)) = &cli.command else {
            panic!("not ota")
        };
        assert!(args.build && args.review.is_none());
        assert_eq!(args.variant, Some(commands::Variant::Fw11Fail));
        assert_eq!(args.python, Some(PathBuf::from("/usr/bin/python3")));
        assert_eq!(args.config, PathBuf::from("private/other.json"));
    }

    #[test]
    fn ota_refuses_the_motion_variant_and_mixed_sources() {
        let error = ota(&["--build", "--variant", "fw16-pulse"])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("motion builds stay manual"), "{error}");

        // --variant and --python without --build are refused by the command.
        for extra in [&["--build", "--review", "private/verification/r"][..], &[]] {
            assert!(ota(extra).is_err(), "{extra:?}");
        }
    }
}
