//! The restricted reader: receive-only observation, ROM identity, bounded and
//! full reads, and status reads. It never programs or erases flash.
//!
//! Port of `main` in `tools/flash/uart-flash-read.py`; its reports keep the
//! same fields, so either implementation's later sessions accept them.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::canonical::sha256;
use crate::error::{Result, require};
use crate::evidence::{self, utc};
use crate::protocol::{self, CAPACITY, HELPER_SHA256, Port, ROM_BYTES, ROM_SHA256};
use crate::session::{self, Context, Outcome, progress};
use crate::status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Observe,
    InspectRom,
    Small,
    Full,
    Status,
}

impl Mode {
    /// The report's `mode`, as the Python reader names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::InspectRom => "inspect-rom",
            Self::Small => "read-small",
            Self::Full => "read-full",
            Self::Status => "read-status",
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
    pub start_km4: bool,
}

/// Two fixed status observations, each kept before stability is checked.
pub fn inspect_status(
    port: &mut dyn Port,
    record: &mut dyn FnMut([u8; 3]) -> Result<()>,
) -> Result<Value> {
    let mut observations = Vec::new();
    for _ in 0..2 {
        let values = status::read_status_registers(port)?;
        observations.push(values);
        record(values)?;
    }

    require(
        observations[0] == observations[1],
        "Status observations differ; stop without retry",
    )?;

    Ok(json!({
        "observations": observations, "matching": true,
        "bank1_assessment": status::assess_range(observations[1], 0x186000, 0x2D6000)?,
        "status_write_commands": false,
        "limit": "Sequential read observations only; physical write permission not established",
    }))
}

pub fn run(context: &Context, args: &Args) -> Result<Outcome> {
    let root = &context.root;

    // Validate local inputs before opening any port or writing any command.
    let mut helper = None;
    let mut candidates = Vec::new();
    let mut continued = None;

    match args.mode {
        Mode::Observe => require(
            (1.0..=180.0).contains(&args.seconds),
            "Observation must be 1-180 seconds",
        )?,

        Mode::InspectRom => {
            require(
                !args.candidate_rom.is_empty(),
                "Explicit candidate ROM is required",
            )?;
            for path in &args.candidate_rom {
                let data = fs::read(path)?;
                require(
                    data.len() == ROM_BYTES && ROM_SHA256.contains(&sha256(&data).as_str()),
                    "Candidate is not a pinned SDK ROM",
                )?;
                candidates.push((path.display().to_string(), data));
            }
        }

        _ => {
            helper = Some(context.helper(args.helper.as_deref())?);
            let rom = args.rom_report.as_deref().ok_or_else(|| {
                crate::Error::Refused(
                    "A reviewed successful ROM inspection report is required".into(),
                )
            })?;
            session::validate_rom_report(context, rom)?;

            if args.mode == Mode::Status {
                continued = Some(session::validate_status_session(
                    context,
                    args.small_report.as_deref(),
                    &args.port,
                )?);
            }

            if args.mode == Mode::Full {
                let small = args.small_report.as_deref().ok_or_else(|| {
                    crate::Error::Refused("A completed bounded read is required first".into())
                })?;
                continued = Some(session::validate_small_for_full(
                    context, small, &args.port,
                )?);
            }
        }
    }

    let directory = root.output_directory(&args.output)?;
    let report_path = directory.join("report.json");

    let metadata = RefCell::new(json!({
        "mode": args.mode.name(), "port": args.port, "baud": 115200, "format": "8N1",
        "start_utc": utc(), "status": "prepared",
        "helper_sha256": helper.as_ref().map(|_| HELPER_SHA256),
        "flash_write_or_erase_commands": false, "capacity_bytes": CAPACITY,
        "tool": "petlibre-flash", "transport": context.transport(), "tool_sha256": context.tool_sha256,
        "limitations": ["No failed-boot recovery established",
                        "Actual ROM side effects not exhaustively verified",
                        "Helper disables OTF; a raw dump may contain ciphertext",
                        "External flash excludes separate OTP/eFuse"],
    }));

    // The prepared report exists before the port opens, so any later stop
    // has a report to update.
    let save = || evidence::save_report(&report_path, &metadata.borrow());
    save()?;

    let result = (|| -> Result<Value> {
        let mut port = context.record(&args.port, &directory)?;
        metadata.borrow_mut()["status"] = json!("running");
        save()?;

        progress(&format!(
            "Recording {} in {}",
            args.mode.name(),
            directory.display()
        ));

        let continuing = |metadata: &mut Value| {
            metadata["helper_upload_attempted"] = json!(false);
            metadata["continuing_helper_session"] =
                json!(args.small_report.as_ref().unwrap().display().to_string());
            metadata["bounded_read_report_sha256"] = json!(sha256(continued.as_ref().unwrap()));
        };

        match args.mode {
            Mode::Observe => protocol::observe(&mut port, args.seconds),
            Mode::InspectRom => protocol::inspect_rom(&mut port, &candidates, args.start_km4),

            Mode::Status => {
                continuing(&mut metadata.borrow_mut());
                metadata.borrow_mut()["status_observations"] = json!([]);
                save()?;
                inspect_status(&mut port, &mut |values| {
                    metadata.borrow_mut()["status_observations"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(values));
                    save()
                })
            }

            Mode::Small | Mode::Full => {
                if args.mode == Mode::Small {
                    // Require the already-observed ROM loader; never auto-enter it.
                    metadata.borrow_mut()["helper_upload_attempted"] = json!(true);
                    save()?;
                    protocol::upload_helper(&mut port, helper.as_ref().unwrap())?;
                    metadata.borrow_mut()["helper_upload_completed"] = json!(true);
                } else {
                    // Continue the bounded read's helper session: uploading over
                    // the running helper would overwrite its own RAM code.
                    continuing(&mut metadata.borrow_mut());
                }
                save()?;
                full_or_small(&mut port, &directory, args.mode)
            }
        }
    })();

    let complete = result.is_ok();
    {
        let mut metadata = metadata.borrow_mut();
        match result {
            Ok(result) => {
                metadata["result"] = result;
                metadata["status"] = json!("complete");
                progress("Completed; evidence retained privately");
            }
            Err(error) => {
                metadata["status"] = json!("failed");
                metadata["error"] = json!(format!("{}: {error}", error.kind()));
                progress(
                    "Stopped; inspect private report and capture. No automatic retry or fallback.",
                );
            }
        }
        metadata["end_utc"] = json!(utc());
        evidence::stream_hashes(&directory, &mut metadata);
    }

    let saved = save();
    let metadata = metadata.into_inner();
    Ok(session::closed(
        complete,
        json!({"status": metadata["status"], "mode": args.mode.name(),
               "output": directory, "result": metadata.get("result"),
               "error": metadata.get("error")}),
        saved,
    ))
}

fn full_or_small(port: &mut dyn Port, directory: &Path, mode: Mode) -> Result<Value> {
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

    require(
        fs::metadata(&partial)?.len() == length as u64,
        "Final file length mismatch",
    )?;

    // Only a complete, length-checked read takes the final name.
    let finished = directory.join("flash-read.bin");
    require(!finished.exists(), "Refusing to overwrite a completed read")?;
    fs::rename(&partial, &finished)?;

    let mut result = Value::Object(transfer.json());
    result["sha256"] = json!(sha256(&fs::read(&finished)?));
    result["offset"] = json!(0);
    Ok(result)
}
