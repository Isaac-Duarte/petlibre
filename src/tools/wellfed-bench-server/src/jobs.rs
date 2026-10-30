//! A job: one bench command line, parsed by the server and run on its bench
//! with the rig's sequences and the suite unchanged.
//!
//! A client posts the command line's words, never a parsed command, and the
//! server parses them here with the same clap definitions the CLI uses
//! ([`JobCommand`], [`JobOptions`]). So every check clap makes (between
//! flags, of ranges, of required names) holds for whatever reaches the
//! socket, and there is one parser for every client.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use wellfed_formats::bench::Config;
use wellfed_formats::bench::banks::Written;
use wellfed_formats::bench::progress::{BenchCommand, ProgressSink, Status};
use wellfed_formats::bench::{CAPTURES, STATUS_DIR, plain_name};
use wellfed_itest::{ItestArgs, StockOtaArgs};
use wellfed_relay::Relay;
use wellfed_rig::Cancel;
use wellfed_rig::commands::{
    self, Bench, ExecuteArgs, GiveWayArgs, GiveWayExecuteArgs, OtaArgs, StockArgs,
    StockExecuteArgs, TransientArgs, UartWhenOff, UpdateArgs, seconds_arg,
};
use wellfed_rig::tools::{Children, Processes};

use wellfed_core::buttons::Button;

use crate::api::Refusal;
use crate::board::SharedBoard;
use crate::feeder::Feeder;
use crate::tool_jobs::{self, Asking, InstallArgs, ToolJobError};

/// A bench command that touches the hardware, as a job.
#[derive(Debug, Clone, Subcommand)]
pub enum JobCommand {
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
    /// Hold a panel button on a running `virtual-panel` build, typed on
    /// its UART: the same gesture as a finger, with nobody at the bench.
    Press {
        /// The button: reset, mic, dispense or lock.
        #[arg(value_parser = wellfed_core::virtual_panel::button)]
        button: Button,
        /// How long it is held, 1 to 15000 ms.
        #[arg(long, default_value_t = wellfed_core::virtual_panel::DEFAULT_HOLD_MS,
              value_parser = clap::value_parser!(u32).range(1..=i64::from(wellfed_core::virtual_panel::MAX_HOLD_MS)))]
        hold_ms: u32,
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
    /// Stock to a replacement with `wellfed install` (DNS route or
    /// `--from-bank0`), its questions answered on the page, with
    /// `wellfed-bench answer` or by an agent. Restarts the
    /// feeder itself with relay 1 when the installer asks for it.
    Install(InstallArgs),
}

impl JobCommand {
    pub fn label(&self) -> BenchCommand {
        match self {
            Self::Off => BenchCommand::Off,
            Self::On => BenchCommand::On,
            Self::Capture { .. } => BenchCommand::Capture,
            Self::Press { .. } => BenchCommand::Press,
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
            Self::Install(_) => BenchCommand::Install,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Off | Self::On | Self::Press { .. } | Self::Install(_) => None,
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

    /// The command and its session name, as the state and console show it.
    pub fn describe(&self) -> String {
        let word = self.label().word();
        match self.name() {
            Some(name) => format!("{word} {name}"),
            None => word.to_string(),
        }
    }

    /// Whether the job holds the UART (a capture, a loader session, a read
    /// or a write) or cold boots into one: the server's feeder session
    /// closes for it. The over-the-air loop and the suite ask through that
    /// session, so they keep it.
    pub(crate) fn holds_uart(&self) -> bool {
        match self {
            Self::Capture { .. }
            | Self::Press { .. }
            | Self::Boot { .. }
            | Self::Loader { .. }
            | Self::Transients(_)
            | Self::Dump { .. }
            | Self::FlashPrepare(_)
            | Self::FlashExecute(_)
            | Self::GiveWayPrepare(_)
            | Self::GiveWayExecute(_)
            | Self::StockPrepare(_)
            | Self::StockExecute(_) => true,
            Self::Off
            | Self::On
            | Self::Ota(_)
            | Self::Itest(_)
            | Self::StockOta(_)
            | Self::Install(_) => false,
        }
    }

    /// Whether the job runs device API sessions of its own throughout:
    /// `stock-ota` meets stock, which serves no device API, then the build
    /// it installs on bank 0. The server's session closes for it rather
    /// than search for a feeder that cannot answer. `install` meets stock
    /// too, then pairs with the replacement under its own key; `wellfed`
    /// opens no session beside the server's unless the server shows it
    /// paused for the job.
    pub(crate) fn own_sessions(&self) -> bool {
        matches!(self, Self::StockOta(_) | Self::Install(_))
    }

    /// One of the integration suite's commands.
    fn suite(&self) -> bool {
        matches!(self, Self::Itest(_) | Self::StockOta(_))
    }

    /// Whether the job may switch relays. The over-the-air loop only
    /// captures, and the suite cold boots only with `--power`: the others
    /// get a board that refuses every switch, as when they ran alone and
    /// never opened it.
    fn switches_relays(&self) -> bool {
        match self {
            Self::Ota(_) => false,
            Self::Itest(args) => args.power,
            Self::StockOta(args) => args.power,
            _ => true,
        }
    }
}

/// The options every job takes, global on the command line.
#[derive(Debug, Clone, Args)]
pub struct JobOptions {
    /// Power-off time before a cold boot, in seconds.
    #[arg(long, global = true, default_value_t = 2.0)]
    pub off_seconds: f64,
    /// What the UART lines (relays 3 and 4) do while the feeder is off.
    #[arg(long, global = true, value_enum, default_value_t = UartWhenOff::Isolated)]
    pub uart_when_off: UartWhenOff,
}

/// A job's command line, as the server parses what a client posts.
#[derive(Debug, Parser)]
#[command(name = "wellfed-bench", no_binary_name = true)]
struct JobLine {
    // No `--root`: the client sends its checkout beside the words, and the
    // server refuses one that is not its own.
    #[command(flatten)]
    options: JobOptions,
    #[command(subcommand)]
    command: JobCommand,
}

/// A parsed job: the command and its options.
#[derive(Debug, Clone)]
pub(crate) struct JobSpec {
    pub(crate) command: JobCommand,
    pub(crate) options: JobOptions,
}

/// Parse a client's words as the CLI does, refusing what is no job.
pub(crate) fn parse(argv: &[String]) -> Result<JobSpec, Refusal> {
    let line = JobLine::try_parse_from(argv).map_err(|error| Refusal::Unparsed {
        message: error.render().to_string(),
    })?;

    // The catalog is the client's to print; as a job it would run the suite.
    if let JobCommand::Itest(args) = &line.command
        && args.list
    {
        return Err(Refusal::NotAJob {
            what: "itest --list".into(),
        });
    }
    if let Some(name) = line.command.name()
        && !plain_name(name)
    {
        return Err(Refusal::BadName { name: name.into() });
    }
    if seconds_arg("--off-seconds", line.options.off_seconds).is_err() {
        return Err(Refusal::BadLength {
            value: line.options.off_seconds,
        });
    }

    Ok(JobSpec {
        command: line.command,
        options: line.options,
    })
}

/// The tool binaries every job runs.
#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub flash_bin: PathBuf,
    pub stock_bin: PathBuf,
    pub build_bin: PathBuf,
    /// `wellfed`, which `install` runs.
    pub wellfed_bin: PathBuf,
}

/// What a job runs on: the server's checkout, config, board, tools and
/// feeder session.
#[derive(Clone)]
pub(crate) struct Bed {
    pub(crate) root: PathBuf,
    pub(crate) config: Config,
    pub(crate) board: SharedBoard,
    pub(crate) tools: ToolPaths,
    pub(crate) feeder: Feeder,
}

/// Why a job stopped before or during its command.
#[derive(Debug, thiserror::Error)]
pub(crate) enum JobError {
    #[error(transparent)]
    Rig(#[from] wellfed_rig::Error),
    #[error(transparent)]
    Suite(#[from] wellfed_itest::Error),
    #[error(transparent)]
    Tool(#[from] ToolJobError),
    /// A bug: the server catches it, so the bench stays served.
    #[error("The job panicked")]
    Panicked,
    #[error("Could not start the job's thread: {0}")]
    Spawn(std::io::Error),
}

/// What one run needs besides its spec: its cancel, its children's
/// registry and where its record goes.
pub(crate) struct Run {
    pub(crate) cancel: Cancel,
    pub(crate) children: Children,
    pub(crate) sink: Box<dyn ProgressSink>,
    /// The job's questions and the answers the server takes for them.
    pub(crate) asking: Asking,
}

/// How a job ended: its exit code, the error it stopped on, and what its
/// sequence wrote to the banks (whatever the code: a write that completed
/// before a later step stopped still happened).
pub(crate) struct Ran {
    pub(crate) code: u8,
    pub(crate) error: Option<JobError>,
    pub(crate) written: Vec<Written>,
}

/// Run `spec` to its end on this thread: open the bench, record the steps,
/// run the command and record how it finished.
pub(crate) fn run(bed: &Bed, spec: &JobSpec, run: Run) -> Ran {
    let stopped = |error| Ran {
        code: 1,
        error: Some(error),
        written: Vec::new(),
    };
    let mut bench = match open(bed, spec, &run) {
        Ok(bench) => bench,
        Err(error) => return stopped(error),
    };
    if let Err(error) = record_start(&mut bench, &spec.command, run.sink) {
        return stopped(error);
    }

    let result = dispatch(&mut bench, &spec.command, &bed.tools, &run.asking);
    let (code, error) = match result {
        Ok(code) => (code, None),
        Err(error) => (1, Some(error)),
    };
    bench
        .status
        .finish(code, error.as_ref().map(ToString::to_string));
    Ran {
        code,
        error,
        written: std::mem::take(&mut bench.written),
    }
}

fn open(bed: &Bed, spec: &JobSpec, run: &Run) -> Result<Bench, JobError> {
    if let Some(name) = spec.command.name() {
        commands::check_name(name)?;
    }
    let off_time: Duration = seconds_arg("--off-seconds", spec.options.off_seconds)?;

    let relay = if spec.command.switches_relays() {
        Relay::of(Box::new(bed.board.clone()))
    } else {
        Relay::detached()
    };
    let tools = Processes::new(
        bed.root.clone(),
        bed.tools.flash_bin.clone(),
        bed.tools.stock_bin.clone(),
        bed.tools.build_bin.clone(),
        run.cancel.clone(),
        run.children.clone(),
    );
    // Every device API request the job makes goes through the server's one
    // session, never a second one of the job's own; but for a job whose
    // sessions are its own, which the server closes its session for.
    let tools = if spec.command.own_sessions() {
        tools
    } else {
        let (keys, link) = bed.feeder.link();
        tools.with_link(keys, Box::new(link))
    };

    Ok(Bench {
        root: bed.root.clone(),
        config: bed.config.clone(),
        relay,
        status: Status::disabled(),
        tools: Box::new(tools),
        children: run.children.clone(),
        cancel: run.cancel.clone(),
        sleep: wellfed_rig::sleep,
        off_time,
        uart_when_off: spec.options.uart_when_off,
        written: Vec::new(),
    })
}

/// Start the command's progress record: its steps and the flash client it
/// runs.
fn record_start(
    bench: &mut Bench,
    command: &JobCommand,
    sink: Box<dyn ProgressSink>,
) -> Result<(), JobError> {
    let label = command.label();
    let steps: Vec<&str> = match command {
        JobCommand::Capture { .. } => vec!["capture"],
        JobCommand::Press { .. } => vec!["wellfed-flash read press"],
        JobCommand::Ota(args) if args.cancel => vec![commands::ota::CANCEL_STEP],
        JobCommand::Ota(args) => commands::ota::steps(args.source()),
        JobCommand::Itest(args) => wellfed_itest::itest_steps(args)?,
        JobCommand::StockOta(args) => wellfed_itest::stock_ota_steps(args),
        JobCommand::StockPrepare(args) if args.fallback.is_some() => {
            commands::steps(BenchCommand::GiveWayPrepare).to_vec()
        }
        JobCommand::StockExecute(args) if args.stock.fallback.is_some() => {
            commands::steps(BenchCommand::GiveWayExecute).to_vec()
        }
        _ => commands::steps(label).to_vec(),
    };

    let status_dir = bench.root.join(STATUS_DIR);
    bench.status = Status::new(&bench.root, status_dir).with_sink(sink);
    bench
        .status
        .begin(label, command.name(), &steps)
        .map_err(wellfed_rig::Error::from)?;
    if command.suite() {
        bench.status.fix_steps();
    }
    if let Some(tool) = bench.tools.flash_tool() {
        bench.status.flash_tool(tool);
    }
    Ok(())
}

/// A press's evidence directory under private/captures/: one per press, by
/// the time it was typed, so a job needs no name.
fn press_name() -> String {
    format!(
        "press-{}",
        wellfed_formats::bench::since_epoch().as_millis()
    )
}

/// Run the command: the suite's on `wellfed-itest`, the rest on the rig.
fn dispatch(
    bench: &mut Bench,
    command: &JobCommand,
    tools: &ToolPaths,
    asking: &Asking,
) -> Result<u8, JobError> {
    let stock_bin = tools.stock_bin.clone();
    let captures = bench.root.join(CAPTURES);
    let ran = match command {
        JobCommand::Off => bench.off(),
        JobCommand::On => bench.on(),
        JobCommand::Capture { name, seconds } => {
            seconds_arg("--seconds", *seconds).and_then(|length| {
                bench.status.step("capture");
                bench.capture(name, length)
            })
        }
        JobCommand::Press { button, hold_ms } => bench.press(&press_name(), *button, *hold_ms),
        JobCommand::Boot { name, seconds } => seconds_arg("--seconds", *seconds)
            .and_then(|length| bench.boot(&captures.join(name), length))
            .map(|_| 0),
        JobCommand::Loader { name } => bench.loader(&captures.join(name)).map(|_| 0),
        JobCommand::Transients(args) => bench.transients(args),
        JobCommand::Dump { name } => bench.dump(name),
        JobCommand::FlashPrepare(args) => bench.flash_prepare(args),
        JobCommand::FlashExecute(args) => bench.flash_execute(args),
        JobCommand::GiveWayPrepare(args) => bench.give_way_prepare(args),
        JobCommand::GiveWayExecute(args) => bench.give_way_execute(args),
        JobCommand::StockPrepare(args) => bench.stock_prepare(args),
        JobCommand::StockExecute(args) => bench.stock_execute(args),
        JobCommand::Ota(args) => bench.ota(args),
        JobCommand::Itest(args) => return Ok(wellfed_itest::itest(bench, args, stock_bin)?),
        JobCommand::StockOta(args) => {
            return Ok(wellfed_itest::stock_ota(bench, args, stock_bin)?);
        }
        JobCommand::Install(args) => return tool_jobs::install(bench, args, tools, asking),
    };
    Ok(ran?)
}

#[cfg(test)]
mod tests {

    use wellfed_relay::{AtBoard, FakeBoard, Switch};

    use super::*;
    use crate::state::Store;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    struct Discard;

    impl ProgressSink for Discard {
        fn publish(&mut self, _: &wellfed_formats::bench::progress::Current) {}
    }

    #[test]
    fn an_over_the_air_job_gets_a_board_that_refuses_every_switch() {
        let board = FakeBoard::default();
        let store = Store::new(crate::tests::idle_state());
        let root = PathBuf::from("/nonexistent");
        let banks = crate::banks::Banks::open(&root, store.clone()).unwrap();
        let notes = crate::notes::Notes::open(&root, store.clone());
        let (feeder, _) = Feeder::new(
            &root,
            &crate::feeder::DeviceOptions::default(),
            store.clone(),
            banks,
            notes,
        );
        let bed = Bed {
            root,
            config: serde_json::from_str(
                r#"{"relay_port": "r", "uart_port": "u", "baseline": "b", "rom_report": "r"}"#,
            )
            .unwrap(),
            board: SharedBoard::new(
                Box::new(AtBoard::new(Box::new(board.clone()))),
                store.clone(),
            ),
            tools: ToolPaths {
                flash_bin: PathBuf::from("absent"),
                stock_bin: PathBuf::from("absent"),
                build_bin: PathBuf::from("absent"),
                wellfed_bin: PathBuf::from("absent"),
            },
            feeder,
        };
        let run = || Run {
            cancel: Cancel::new(),
            children: Children::new(),
            sink: Box::new(Discard),
            asking: crate::tool_jobs::asking(1, store.clone()).0,
        };

        for argv in [&["ota", "o1", "--build"][..], &["itest", "i1"]] {
            let spec = parse(&words(argv)).unwrap();
            let mut bench = open(&bed, &spec, &run()).unwrap();
            assert!(bench.relay.set(1, Switch::On).is_err(), "{argv:?}");
        }
        assert!(board.log().is_empty());

        // With --power the suite may cold boot, so it holds the real board.
        let spec = parse(&words(&["itest", "i1", "--power"])).unwrap();
        let mut bench = open(&bed, &spec, &run()).unwrap();
        bench.relay.set(1, Switch::On).unwrap();
        assert_eq!(board.log(), [(1, true)]);
    }
}
