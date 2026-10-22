//! `petlibre-flash`: the UART flash client.
//!
//! Progress goes to stderr; stdout carries exactly one JSON summary, so a
//! script or agent can act on the result. Exit status: 0 complete, 1 stopped
//! after the port was opened (inspect the evidence; nothing is retried), 2
//! refused before any serial access.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use petlibre_flash::protocol::INTERRUPTED;
use petlibre_flash::session::{Context, Outcome};
use petlibre_flash::{read, stock, update};

#[derive(Parser)]
#[command(
    version,
    about = "UART flash reads, development updates and stock recovery for the PLAF103 research unit",
    long_about = "UART flash reads, development updates and stock recovery for the PLAF103 research unit.\n\n\
        Every device command continues the supervised bench procedure in docs/development/uart-development.md: \
        nothing here enters the loader, controls power, retries or boots. Progress goes to stderr and one JSON \
        summary to stdout. Exit status: 0 complete, 1 stopped after the port opened, 2 refused before serial access."
)]
struct Cli {
    /// Repository checkout whose ignored private/ holds all evidence.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Restricted reads that never program or erase flash.
    Read(ReadArgs),
    /// Region-scoped development update of bank 1 (default) or bank 0.
    Update(UpdateArgs),
    /// Clear bank 1's first marker byte so the loader falls back to bank 0.
    GiveWay(GiveWayArgs),
    /// Exact original bank-1 stock recovery from a full read.
    StockRecovery(StockArgs),
    /// Offline: plan stock recovery sectors from a baseline and current read.
    PlanStockRecovery {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        current: PathBuf,
        /// New private directory for plan.json and its sector payloads.
        #[arg(long)]
        output: PathBuf,
    },
    /// Offline: decode GD25Q64E status registers and assess a write range.
    DecodeStatus {
        #[arg(long, value_parser = number::<u8>)]
        sr1: u8,
        #[arg(long, value_parser = number::<u8>)]
        sr2: u8,
        #[arg(long, value_parser = number::<u8>)]
        sr3: u8,
        #[arg(long, value_parser = number::<usize>, default_value = "0x186000")]
        start: usize,
        #[arg(long, value_parser = number::<usize>, default_value = "0x2d6000")]
        end: usize,
    },
    /// Offline: check a transaction journal's hash chain; never resumes it.
    InspectJournal { journal: PathBuf },
}

fn number<T: TryFrom<u64>>(text: &str) -> Result<T, String> {
    let value = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|error| error.to_string())?;
    T::try_from(value).map_err(|_| format!("{text} is out of range"))
}

#[derive(Clone, Copy, ValueEnum)]
enum ReadMode {
    /// Receive only, for --seconds.
    Observe,
    /// Sample the ROM through the stock console before any helper upload.
    InspectRom,
    /// Upload the pinned helper to the observed loader and read 4 KiB.
    Small,
    /// Continue the bounded read's helper session and read all 8 MiB.
    Full,
    /// Continue the bounded read's helper session and read the status registers.
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum Core {
    Km4,
    Km0,
}

#[derive(Args)]
struct ReadArgs {
    mode: ReadMode,
    #[arg(long)]
    port: String,
    /// New private output directory.
    #[arg(long)]
    output: PathBuf,
    /// Pinned vendor helper (default: the SDK's imgtool_flashloader_amebad.bin under private/).
    #[arg(long)]
    helper: Option<PathBuf>,
    /// Pinned SDK ROM image to compare the sampled words with (repeatable).
    #[arg(long)]
    candidate_rom: Vec<PathBuf>,
    #[arg(long)]
    rom_report: Option<PathBuf>,
    #[arg(long)]
    small_report: Option<PathBuf>,
    /// Receive-only observation duration (1-180 seconds).
    #[arg(long, default_value_t = 60.0)]
    seconds: f64,
    /// Known console owner for ROM inspection; never inferred on a mismatch.
    #[arg(long, value_enum, default_value = "km4")]
    start_core: Core,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Phase {
    /// Read the region and guards; write nothing.
    Inspect,
    /// Write a reviewed plan from a completed inspection; no serial access.
    Prepare,
    /// Execute exactly the reviewed plan.
    Execute,
}

#[derive(Args)]
struct EvidenceArgs {
    #[arg(long)]
    port: String,
    /// The pinned original full-flash backup.
    #[arg(long)]
    baseline: PathBuf,
    #[arg(long)]
    rom_report: PathBuf,
    /// The bounded read whose helper session this continues.
    #[arg(long)]
    small_report: PathBuf,
    #[arg(long)]
    helper: Option<PathBuf>,
    /// New private output: a directory, or the plan file when preparing.
    #[arg(long)]
    output: PathBuf,
    /// Completed inspection report (prepare and execute).
    #[arg(long)]
    inspection: Option<PathBuf>,
    /// Reviewed plan (execute).
    #[arg(long)]
    plan: Option<PathBuf>,
    #[arg(long)]
    expected_plan_sha256: Option<String>,
}

impl EvidenceArgs {
    fn evidence(self) -> update::Evidence {
        update::Evidence {
            port: self.port,
            baseline: self.baseline,
            rom_report: self.rom_report,
            small_report: self.small_report,
            helper: self.helper,
            output: self.output,
            inspection: self.inspection,
            plan: self.plan,
            expected_plan_sha256: self.expected_plan_sha256,
        }
    }
}

fn phase(phase: Phase) -> update::Mode {
    match phase {
        Phase::Inspect => update::Mode::Inspect,
        Phase::Prepare => update::Mode::Prepare,
        Phase::Execute => update::Mode::Execute,
    }
}

#[derive(Args)]
struct UpdateArgs {
    phase: Phase,
    #[command(flatten)]
    evidence: EvidenceArgs,
    /// Frozen review directory of the image to install.
    #[arg(long)]
    target: PathBuf,
    /// Frozen review of the installed image; bank 0 may omit it with --original-read.
    #[arg(long)]
    source: Option<PathBuf>,
    /// Bank 0 without a source: a private full read pinning bank 0's current bytes.
    #[arg(long)]
    original_read: Option<PathBuf>,
    /// Bank to update; 0 is the factory loader's fallback (bank 1 must stay valid).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=1))]
    bank: u8,
    /// Bank 1 after a give-way: the source with only its first marker byte cleared.
    #[arg(long)]
    restore_marker: bool,
    /// Finish a stopped update: staged marker, every sector source, staged target or erased.
    #[arg(long)]
    interrupted: bool,
}

#[derive(Args)]
struct GiveWayArgs {
    phase: Phase,
    #[command(flatten)]
    evidence: EvidenceArgs,
    /// Frozen review of the image bank 0 must hold exactly.
    #[arg(long)]
    fallback: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StockPhase {
    Prepare,
    Execute,
}

#[derive(Args)]
struct StockArgs {
    phase: StockPhase,
    #[command(flatten)]
    evidence: EvidenceArgs,
    /// The complete full read (flash-read.bin) of this helper session.
    #[arg(long)]
    current: PathBuf,
}

fn device(
    root: &Path,
    run: impl FnOnce(&Context) -> petlibre_flash::Result<Outcome>,
) -> (u8, Value) {
    let context = match Context::new(root) {
        Ok(context) => context,
        Err(error) => return refused(&error),
    };
    match run(&context) {
        Ok(outcome) => (u8::from(!outcome.complete), outcome.summary),
        Err(error) => refused(&error),
    }
}

fn refused(error: &petlibre_flash::Error) -> (u8, Value) {
    eprintln!("Refused before serial access: {error}");
    (
        2,
        json!({"status": "refused", "error_type": error.kind(), "error": error.to_string()}),
    )
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // An operator interrupt stops the session terminally and records it.
    let _ = ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst));
    let root = cli.root.as_path();
    let (code, summary) = match cli.command {
        Command::Read(args) => device(root, |context| {
            let mode = match args.mode {
                ReadMode::Observe => read::Mode::Observe,
                ReadMode::InspectRom => read::Mode::InspectRom,
                ReadMode::Small => read::Mode::Small,
                ReadMode::Full => read::Mode::Full,
                ReadMode::Status => read::Mode::Status,
            };
            read::run(
                context,
                &read::Args {
                    mode,
                    port: args.port,
                    output: args.output,
                    helper: args.helper,
                    candidate_rom: args.candidate_rom,
                    rom_report: args.rom_report,
                    small_report: args.small_report,
                    seconds: args.seconds,
                    start_km4: matches!(args.start_core, Core::Km4),
                },
            )
        }),
        Command::Update(args) => device(root, |context| {
            update::run(
                context,
                &update::Args {
                    mode: phase(args.phase),
                    evidence: args.evidence.evidence(),
                    target: args.target,
                    source: args.source,
                    original_read: args.original_read,
                    bank: args.bank,
                    restore_marker: args.restore_marker,
                    interrupted: args.interrupted,
                },
            )
        }),
        Command::GiveWay(args) => device(root, |context| {
            update::give_way(
                context,
                &update::GiveWayArgs {
                    mode: phase(args.phase),
                    evidence: args.evidence.evidence(),
                    fallback: args.fallback,
                },
            )
        }),
        Command::StockRecovery(args) => device(root, |context| {
            stock::run(
                context,
                &stock::Args {
                    execute: args.phase == StockPhase::Execute,
                    evidence: args.evidence.evidence(),
                    current: args.current,
                },
            )
        }),
        Command::PlanStockRecovery {
            baseline,
            current,
            output,
        } => device(root, |context| {
            stock::plan_offline(context, &baseline, &current, &output).map(|summary| Outcome {
                complete: true,
                summary,
            })
        }),
        Command::DecodeStatus {
            sr1,
            sr2,
            sr3,
            start,
            end,
        } => match petlibre_flash::status::assess_range([sr1, sr2, sr3], start, end) {
            Ok(value) => (0, value),
            Err(error) => refused(&error),
        },
        Command::InspectJournal { journal } => {
            match std::fs::read(&journal)
                .map_err(petlibre_flash::Error::from)
                .and_then(|data| petlibre_flash::journal::inspect(&data))
            {
                Ok(value) => (u8::from(value["verified_terminal"] != true), value),
                Err(error) => refused(&error),
            }
        }
    };
    println!("{summary}");
    ExitCode::from(code)
}
