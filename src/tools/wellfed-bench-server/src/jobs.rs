//! A job: one bench command, as the client's command line parsed it, run on
//! the server's bench with the rig's sequences and the suite unchanged.
//!
//! The command line and the socket share [`JobCommand`]: the CLI parses it
//! with clap and posts it as JSON, and the server runs it on a thread of its
//! own. Clap's checks between flags (`--build` against `--review`,
//! `--destructive` needing `--power`) run in the client; the server trusts
//! the socket's owner, whom its mode limits to the checkout's.

use std::path::PathBuf;
use std::time::Duration;

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use wellfed_formats::bench::Config;
use wellfed_formats::bench::progress::{BenchCommand, ProgressSink, Status};
use wellfed_formats::bench::{CAPTURES, STATUS_DIR};
use wellfed_itest::{ItestArgs, StockOtaArgs};
use wellfed_relay::Relay;
use wellfed_rig::Cancel;
use wellfed_rig::commands::{
    self, Bench, ExecuteArgs, GiveWayArgs, GiveWayExecuteArgs, OtaArgs, StockArgs,
    StockExecuteArgs, TransientArgs, UartWhenOff, UpdateArgs, seconds_arg,
};
use wellfed_rig::tools::{Children, Processes};

use crate::board::SharedBoard;

/// A bench command that touches the hardware, as a job.
#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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

impl JobCommand {
    pub fn label(&self) -> BenchCommand {
        match self {
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

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Off | Self::On => None,
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

/// A job as a client posts it: the command and the options that were the
/// command line's global flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub command: JobCommand,
    /// Power-off time before a cold boot, in seconds.
    pub off_seconds: f64,
    pub uart_when_off: UartWhenOff,
}

/// The tool binaries every job runs.
#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub flash_bin: PathBuf,
    pub stock_bin: PathBuf,
    pub build_bin: PathBuf,
}

/// What a job runs on: the server's checkout, config, board and tools.
#[derive(Clone)]
pub(crate) struct Bed {
    pub(crate) root: PathBuf,
    pub(crate) config: Config,
    pub(crate) board: SharedBoard,
    pub(crate) tools: ToolPaths,
}

/// Why a job stopped before or during its command.
#[derive(Debug, thiserror::Error)]
pub(crate) enum JobError {
    #[error(transparent)]
    Rig(#[from] wellfed_rig::Error),
    #[error(transparent)]
    Suite(#[from] wellfed_itest::Error),
}

/// What one run needs besides its spec: its cancel, its children's
/// registry and where its record goes.
pub(crate) struct Run {
    pub(crate) cancel: Cancel,
    pub(crate) children: Children,
    pub(crate) sink: Box<dyn ProgressSink>,
}

/// Run `spec` to its end on this thread: open the bench, record the steps,
/// run the command and record how it finished. Returns the exit code and
/// the error it stopped on.
pub(crate) fn run(bed: &Bed, spec: &JobSpec, run: Run) -> (u8, Option<String>) {
    let mut bench = match open(bed, spec, &run) {
        Ok(bench) => bench,
        Err(error) => return (1, Some(error.to_string())),
    };
    if let Err(error) = record_start(&mut bench, &spec.command, run.sink) {
        return (1, Some(error.to_string()));
    }

    let result = dispatch(&mut bench, &spec.command, bed.tools.stock_bin.clone());
    let (code, error) = match result {
        Ok(code) => (code, None),
        Err(error) => (1, Some(error.to_string())),
    };
    bench.status.finish(code, error.clone());
    (code, error)
}

fn open(bed: &Bed, spec: &JobSpec, run: &Run) -> Result<Bench, JobError> {
    if let Some(name) = spec.command.name() {
        commands::check_name(name)?;
    }
    let off_time: Duration = seconds_arg("--off-seconds", spec.off_seconds)?;

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
        uart_when_off: spec.uart_when_off,
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

/// Run the command: the suite's on `wellfed-itest`, the rest on the rig.
fn dispatch(bench: &mut Bench, command: &JobCommand, stock_bin: PathBuf) -> Result<u8, JobError> {
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
    };
    Ok(ran?)
}
