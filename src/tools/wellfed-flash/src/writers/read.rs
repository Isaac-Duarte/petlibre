//! The restricted reader: receive-only observation, ROM identity, bounded and
//! full reads, and status reads. It never programs or erases flash.
//!
//! Port of `main` in `src/tools/flash/uart-flash-read.py`; its reports keep the
//! same fields, so either implementation's later sessions accept them.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use crate::evidence::report::{Session, Status, Transport};
use crate::evidence::session::{self, Completion, Context, Outcome, progress};
use crate::link::protocol::{
    self, CAPACITY, Core, HELPER_SHA256, Observation, Port, ROM_BYTES, ROM_SHA256, RomInspection,
    Transfer,
};
use crate::link::status::{self, Assessment};
use wellfed_formats::canonical::sha256;
use wellfed_formats::require;
use wellfed_formats::session::Mode as ReportMode;

/// The reader's own refusals: missing or unpinned inputs before the port
/// opens, and unstable status or an unfinished read file after.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Two status observations required")]
    StatusObservations { count: usize },
    #[error("Status observations differ; stop without retry")]
    StatusDiffers { first: [u8; 3], second: [u8; 3] },
    #[error("Explicit candidate ROM is required")]
    CandidateMissing,
    #[error("Candidate is not a pinned SDK ROM")]
    CandidateUnpinned { path: PathBuf },
    #[error("A reviewed successful ROM inspection report is required")]
    RomReportMissing,
    #[error("A completed bounded read is required first")]
    BoundedReadMissing,
    #[error("The pinned helper is required")]
    HelperMissing,
    #[error("Final file length mismatch")]
    FinalLength { bytes: u64, length: u64 },
    #[error("Refusing to overwrite a completed read")]
    WouldOverwrite { path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
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

impl Mode {
    /// The report's `mode`, as the Python reader names it.
    pub fn report(self) -> ReportMode {
        match self {
            Self::Observe => ReportMode::Observe,
            Self::InspectRom => ReportMode::InspectRom,
            Self::Small => ReportMode::ReadSmall,
            Self::Full => ReportMode::ReadFull,
            Self::Status => ReportMode::ReadStatus,
        }
    }
}

pub struct Args {
    pub mode: Mode,
    pub port: String,
    pub output: PathBuf,
    pub helper: Option<PathBuf>,
    pub candidate_rom: Vec<PathBuf>,
    pub rom_report: Option<PathBuf>,
    pub small_report: Option<PathBuf>,
    pub seconds: f64,
    /// Which core owns the console when ROM inspection starts.
    pub start_core: Core,
}

/// Two matching status observations and what they permit for bank 1.
#[derive(Debug, Clone, Serialize)]
pub struct StatusInspection {
    pub observations: Vec<[u8; 3]>,
    pub matching: bool,
    pub bank1_assessment: Assessment,
    pub status_write_commands: bool,
    pub limit: &'static str,
}

/// A bounded or full read into `flash-read.bin`.
#[derive(Debug, Clone, Serialize)]
pub struct FlashRead {
    #[serde(flatten)]
    pub transfer: Transfer,
    pub sha256: String,
    pub offset: usize,
}

/// A read session's result, by mode.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Reading {
    Observation(Observation),
    Rom(RomInspection),
    Status(StatusInspection),
    Flash(FlashRead),
}

/// The reader's report fields beyond the shared lifecycle.
#[derive(Debug, Serialize)]
struct Details {
    mode: ReportMode,
    port: String,
    baud: u32,
    format: &'static str,
    helper_sha256: Option<&'static str>,
    flash_write_or_erase_commands: bool,
    capacity_bytes: usize,
    tool: &'static str,
    transport: Transport,
    tool_sha256: String,
    limitations: [&'static str; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    helper_upload_attempted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    helper_upload_completed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continuing_helper_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bounded_read_report_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_observations: Option<Vec<[u8; 3]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Reading>,
}

#[derive(Serialize)]
struct Summary<'a> {
    status: Status,
    mode: ReportMode,
    output: &'a Path,
    result: Option<Reading>,
    error: Option<String>,
}

/// Two fixed status observations, each kept before stability is checked.
pub fn inspect_status(
    port: &mut dyn Port,
    record: &mut dyn FnMut([u8; 3]) -> Result<()>,
) -> Result<StatusInspection> {
    let mut observations = Vec::new();
    for _ in 0..2 {
        let values = status::read_status_registers(port)?;
        observations.push(values);
        record(values)?;
    }

    let [first, second] = observations[..] else {
        return Err(Error::StatusObservations {
            count: observations.len(),
        }
        .into());
    };
    require(first == second, || Error::StatusDiffers { first, second })?;

    Ok(StatusInspection {
        bank1_assessment: status::assess_range(second, 0x186000, 0x2D6000)?,
        observations,
        matching: true,
        status_write_commands: false,
        limit: "Sequential read observations only; physical write permission not established",
    })
}

pub fn run(context: &Context, args: &Args) -> Result<Outcome> {
    let root = &context.root;

    // Validate local inputs before opening any port or writing any command.
    let mut helper = None;
    let mut candidates = Vec::new();
    let mut continued = None;

    match args.mode {
        Mode::Observe => protocol::observation_window(args.seconds)?,

        Mode::InspectRom => {
            require(!args.candidate_rom.is_empty(), || Error::CandidateMissing)?;
            for path in &args.candidate_rom {
                let data = fs::read(path)?;
                require(
                    data.len() == ROM_BYTES && ROM_SHA256.contains(&sha256(&data).as_str()),
                    || Error::CandidateUnpinned { path: path.clone() },
                )?;
                candidates.push((path.display().to_string(), data));
            }
        }

        Mode::Small | Mode::Full | Mode::Status => {
            helper = Some(context.helper(args.helper.as_deref())?);
            let Some(rom) = args.rom_report.as_deref() else {
                return Err(Error::RomReportMissing.into());
            };
            session::validate_rom_report(context, rom)?;

            if args.mode == Mode::Status {
                let raw = session::validate_status_session(
                    context,
                    args.small_report.as_deref(),
                    &args.port,
                )?;
                continued = args.small_report.as_deref().map(|path| (path, raw));
            }

            if args.mode == Mode::Full {
                let Some(small) = args.small_report.as_deref() else {
                    return Err(Error::BoundedReadMissing.into());
                };
                let raw = session::validate_small_for_full(context, small, &args.port)?;
                continued = Some((small, raw));
            }
        }
    }

    let directory = root.output_directory(&args.output)?;

    // The prepared report exists before the port opens, so any later stop
    // has a report to update.
    let report = Session::open(
        directory.join("report.json"),
        Details {
            mode: args.mode.report(),
            port: args.port.clone(),
            baud: 115200,
            format: "8N1",
            helper_sha256: helper.as_ref().map(|_| HELPER_SHA256),
            flash_write_or_erase_commands: false,
            capacity_bytes: CAPACITY,
            tool: "wellfed-flash",
            transport: context.transport(),
            tool_sha256: context.tool_sha256.clone(),
            limitations: [
                "No failed-boot recovery established",
                "Actual ROM side effects not exhaustively verified",
                "Helper disables OTF; a raw dump may contain ciphertext",
                "External flash excludes separate OTP/eFuse",
            ],
            helper_upload_attempted: None,
            helper_upload_completed: None,
            continuing_helper_session: None,
            bounded_read_report_sha256: None,
            status_observations: None,
            result: None,
        },
    )?;

    let result = (|| -> Result<Reading> {
        let mut port = context.record(&args.port, &directory)?;
        report.update(|r| r.status = Status::Running)?;

        progress(&format!(
            "Recording {} in {}",
            args.mode.report(),
            directory.display()
        ));

        let continuing = |details: &mut Details| {
            details.helper_upload_attempted = Some(false);
            if let Some((path, raw)) = &continued {
                details.continuing_helper_session = Some(path.display().to_string());
                details.bounded_read_report_sha256 = Some(sha256(raw));
            }
        };

        match args.mode {
            Mode::Observe => protocol::observe(&mut port, args.seconds).map(Reading::Observation),
            Mode::InspectRom => {
                protocol::inspect_rom(&mut port, &candidates, args.start_core).map(Reading::Rom)
            }

            Mode::Status => {
                report.update(|r| {
                    continuing(&mut r.details);
                    r.details.status_observations = Some(Vec::new());
                })?;
                inspect_status(&mut port, &mut |values| {
                    report.update(|r| {
                        r.details
                            .status_observations
                            .get_or_insert_default()
                            .push(values);
                    })
                })
                .map(Reading::Status)
            }

            Mode::Small | Mode::Full => {
                if args.mode == Mode::Small {
                    let Some(helper) = &helper else {
                        return Err(Error::HelperMissing.into());
                    };

                    // Require the already-observed ROM loader; never auto-enter it.
                    report.update(|r| r.details.helper_upload_attempted = Some(true))?;
                    protocol::upload_helper(&mut port, helper)?;
                    report.update(|r| r.details.helper_upload_completed = Some(true))?;
                } else {
                    // Continue the bounded read's helper session: uploading over
                    // the running helper would overwrite its own RAM code.
                    report.update(|r| continuing(&mut r.details))?;
                }
                full_or_small(&mut port, &directory, args.mode).map(Reading::Flash)
            }
        }
    })();

    let completion = match &result {
        Ok(_) => Completion::Complete,
        Err(_) => Completion::Stopped,
    };
    let (report, saved) = report.close(&directory, |r| match result {
        Ok(result) => {
            r.details.result = Some(result);
            r.status = Status::Complete;
            progress("Completed; evidence retained privately");
        }
        Err(error) => {
            r.stop(Status::Failed, &error);
            progress(
                "Stopped; inspect private report and capture. No automatic retry or fallback.",
            );
        }
    });

    let summary = Summary {
        status: report.status,
        mode: args.mode.report(),
        output: &directory,
        result: report.details.result,
        error: report.error,
    };
    Ok(session::closed(completion, &summary, saved))
}

fn full_or_small(port: &mut dyn Port, directory: &Path, mode: Mode) -> Result<FlashRead> {
    let length = if mode == Mode::Small { 4096 } else { CAPACITY };
    let partial = directory.join("flash-read.bin.part");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)?;
    let mut report = |done: usize, total: usize| progress(&format!("Read {done}/{total} bytes"));
    let transfer = protocol::receive_flash(port, 0, length, &mut output, Some(&mut report))?;
    output.sync_all()?;
    drop(output);

    let bytes = fs::metadata(&partial)?.len();
    require(bytes == length as u64, || Error::FinalLength {
        bytes,
        length: length as u64,
    })?;

    // Only a complete, length-checked read takes the final name.
    let finished = directory.join("flash-read.bin");
    require(!finished.exists(), || Error::WouldOverwrite {
        path: finished.clone(),
    })?;
    fs::rename(&partial, &finished)?;

    Ok(FlashRead {
        transfer,
        sha256: sha256(&fs::read(&finished)?),
        offset: 0,
    })
}
