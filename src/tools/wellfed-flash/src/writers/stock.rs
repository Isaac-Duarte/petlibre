//! Prepare or execute exact original bank-1 recovery on the research unit.
//!
//! Port of the `prepare`/`execute` modes of the former
//! `src/tools/flash/uart-stock-recovery.py`, which did the physical stock
//! round trip; this port did it again on October 23, 2026.
//! It continues an established pinned helper session from a complete full
//! read; it never uploads, changes status, enters a loader or boots. An
//! invalid target marker is required for a changed-bank recovery. The Python
//! client's write-back acceptance-test modes were retired, not ported.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::error::{Result, refuse, require};
use crate::evidence;
use crate::evidence::report::{Session, Status, Transport};
use crate::evidence::session::{self, Completion, Context, Outcome, ReadResult, Saved, progress};
use crate::link::protocol::{self, CAPACITY, HELPER_SHA256, Port, Transfer};
use crate::link::sector::Baseline;
use crate::writers::recovery::{self, Plan};
use crate::writers::update::Evidence;
use wellfed_formats::canonical::{canonical, sha256};
use wellfed_formats::flash_plan::{Kind, ToolHashes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Phase {
    /// Write a reviewed plan from a complete full read; no serial access.
    Prepare,
    /// Execute exactly the reviewed plan.
    Execute,
}

pub struct Args {
    pub phase: Phase,
    pub evidence: Evidence,
    /// A complete full read from this helper session, beside its report.
    pub current: PathBuf,
}

/// The reviewed stock-recovery plan: the offline plan and the session
/// evidence it continues.
#[derive(Debug, Clone, Serialize)]
pub struct Reviewed {
    pub schema_version: u32,
    pub kind: Kind,
    pub port: String,
    pub recovery_plan: Plan,
    pub current_report_sha256: String,
    pub bounded_read_report_sha256: String,
    pub rom_report_sha256: String,
    pub tool_sha256: ToolHashes,
}

/// All local provenance checks run before serial access or output creation.
fn prepare_inputs(context: &Context, args: &Args) -> Result<(Baseline, Vec<u8>, Reviewed)> {
    let root = &context.root;
    let evidence = &args.evidence;
    let baseline = context.baseline(&evidence.baseline)?;
    let current = root.private_file(&args.current)?;
    let plan = recovery::plan(&baseline, &current)?;

    context.helper(evidence.helper.as_deref())?;
    let rom = session::validate_rom_report(context, &evidence.rom_report)?;
    let small =
        session::validate_status_session(context, Some(&evidence.small_report), &evidence.port)?;

    let report_path = args
        .current
        .parent()
        .map(|p| p.join("report.json"))
        .unwrap_or_default();
    let raw_report = root.private_file(&report_path)?;
    let report: Saved<ReadResult> = serde_json::from_slice(&raw_report)?;
    context.same_transport(report.transport)?;

    let result = report.result.as_ref();
    require(
        report.status == Some(Status::Complete)
            && report.mode.as_deref() == Some("read-full")
            && report.port.as_deref() == Some(evidence.port.as_str())
            && report.helper_sha256.as_deref() == Some(HELPER_SHA256)
            && report.helper_upload_attempted == Some(false)
            && report.bounded_read_report_sha256.as_deref() == Some(&sha256(&small))
            && result.and_then(|r| r.offset) == Some(0)
            && result.and_then(|r| r.bytes) == u64::try_from(CAPACITY).ok()
            && result.and_then(|r| r.sha256.as_deref()) == Some(&sha256(&current)),
        "Complete full-read report from this helper session required",
    )?;
    session::streams_match(
        root,
        &report_path,
        &report.streams,
        "Full-read serial evidence hash mismatch",
    )?;

    let reviewed = Reviewed {
        schema_version: 1,
        kind: Kind::StockRecovery,
        port: evidence.port.clone(),
        recovery_plan: plan,
        current_report_sha256: sha256(&raw_report),
        bounded_read_report_sha256: sha256(&small),
        rom_report_sha256: sha256(&rom),
        tool_sha256: context.tools(),
    };

    Ok((baseline, current, reviewed))
}

/// Everything but the nested planner's creation time, never a guard. The
/// reviewed file is compared as written, so this works on its JSON.
fn comparable(reviewed: &Value) -> Result<Vec<u8>> {
    let mut result = reviewed.clone();
    let plan = result
        .get_mut("recovery_plan")
        .and_then(Value::as_object_mut);
    let Some(plan) = plan.filter(|plan| plan.get("created_utc").is_some_and(Value::is_string))
    else {
        return refuse("Plan creation time required");
    };

    plan.remove("created_utc");
    Ok(canonical(&result)?)
}

#[derive(Serialize)]
struct Prepared<'a> {
    status: &'static str,
    operation: Kind,
    plan: &'a Path,
    plan_sha256: String,
    changed_sectors: usize,
}

pub fn run(context: &Context, args: &Args) -> Result<Outcome> {
    let (baseline, current, expected) = prepare_inputs(context, args)?;
    let evidence = &args.evidence;

    if args.phase == Phase::Prepare {
        let sha = session::write_plan(&context.root, &evidence.output, &expected)?;
        progress(&format!("Prepared bank-1 recovery plan SHA256: {sha}"));
        return Outcome::done(&Prepared {
            status: "prepared",
            operation: Kind::StockRecovery,
            plan: &evidence.output,
            plan_sha256: sha,
            changed_sectors: expected.recovery_plan.changed_sector_count,
        });
    }

    let (Some(plan), Some(expected_sha)) = (&evidence.plan, &evidence.expected_plan_sha256) else {
        return refuse("Exact reviewed plan/hash required");
    };

    let raw = context.root.private_file(plan)?;
    require(
        sha256(&raw) == *expected_sha,
        "Reviewed plan SHA256 differs",
    )?;

    let reviewed: Value = serde_json::from_slice(&raw)?;
    require(
        comparable(&reviewed)? == comparable(&serde_json::to_value(&expected)?)?,
        "Reviewed plan/source/evidence differs",
    )?;

    let manifest = &reviewed["recovery_plan"];
    recovery::validate_plan(&baseline, &current, manifest)?;
    execute(context, args, &baseline, &current, manifest, &raw)
}

/// One full snapshot as the session report lists it.
#[derive(Debug, Clone, Serialize)]
struct Snapshot {
    #[serde(flatten)]
    transfer: Transfer,
    file: String,
    sha256: String,
}

/// The stock session's report fields beyond the shared lifecycle.
#[derive(Serialize)]
struct Details {
    port: String,
    operation: Kind,
    reviewed_plan_sha256: String,
    helper_upload_attempted: bool,
    snapshots: Vec<Snapshot>,
    transaction_directory: &'static str,
    physical_recovery_verified: bool,
    tool: &'static str,
    transport: Transport,
    scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_sha256: Option<String>,
}

#[derive(Serialize)]
struct Summary<'a> {
    status: Status,
    operation: Kind,
    output: &'a Path,
    snapshot_sha256: Option<String>,
    error: Option<String>,
}

/// Open once; the transaction owns durable mutation intent and activation order.
fn execute(
    context: &Context,
    args: &Args,
    baseline: &Baseline,
    current: &[u8],
    manifest: &Value,
    raw: &[u8],
) -> Result<Outcome> {
    let directory = context.root.output_directory(&args.evidence.output)?;
    session::keep_plan(&directory, raw)?;
    if let Some(parent) = directory.parent() {
        evidence::sync_directory(parent)?;
    }

    let state = Session::open(
        directory.join("session.json"),
        Details {
            port: args.evidence.port.clone(),
            operation: Kind::StockRecovery,
            reviewed_plan_sha256: sha256(raw),
            helper_upload_attempted: false,
            snapshots: Vec::new(),
            transaction_directory: "transaction",
            physical_recovery_verified: false,
            tool: "wellfed-flash",
            transport: context.transport(),
            scope: "Supervised serial stock restore; physical acceptance requires external bench evidence",
            snapshot_sha256: None,
        },
    )?;

    let result = (|| -> Result<recovery::Recovered> {
        let mut port = context.record(&args.evidence.port, &directory)?;
        state.update(|s| s.status = Status::Running)?;

        progress("Original bank-1 operation; fresh full verification before any mutation");

        let mut snapshot = |port: &mut dyn Port| -> Result<Vec<u8>> {
            let number = state.read(|s| s.details.snapshots.len()) + 1;
            let partial = directory.join(format!("snapshot-{number:02}.bin.part"));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)?;
            let mut report =
                |n: usize, total: usize| progress(&format!("Snapshot {number}: {n}/{total}"));
            let transfer =
                protocol::receive_flash(port, 0, CAPACITY, &mut file, Some(&mut report))?;
            file.sync_all()?;
            drop(file);

            let data = fs::read(&partial)?;
            require(data.len() == CAPACITY, "Full snapshot truncated")?;
            let name = format!("snapshot-{number:02}.bin");
            fs::rename(&partial, directory.join(&name))?;
            evidence::sync_directory(&directory)?;

            let entry = Snapshot {
                transfer,
                file: name,
                sha256: sha256(&data),
            };
            state.update(|s| s.details.snapshots.push(entry))?;
            Ok(data)
        };

        recovery::run_transaction(
            &mut port,
            &context.root,
            baseline,
            current,
            manifest,
            &mut snapshot,
            &directory.join("transaction"),
        )
    })();

    let completion = match &result {
        Ok(_) => Completion::Complete,
        Err(_) => Completion::Stopped,
    };
    let (state, saved) = state.close(&directory, |s| match result {
        Ok(report) => {
            s.status = Status::Verified;
            s.details.snapshot_sha256 = Some(report.snapshot_sha256);
            progress(
                "Bank-1 recovery and preserved ranges verified; keep application boot excluded until review",
            );
        }
        Err(error) => {
            s.stop(Status::Stopped, &error);
            progress(
                "Stopped without replay. Keep application boot excluded and review private evidence.",
            );
        }
    });

    let summary = Summary {
        status: state.status,
        operation: Kind::StockRecovery,
        output: &directory,
        snapshot_sha256: state.details.snapshot_sha256,
        error: state.error,
    };
    Ok(session::closed(completion, &summary, saved))
}

#[derive(Serialize)]
struct Planned<'a> {
    status: &'static str,
    output: &'a Path,
    changed_sector_count: usize,
}

/// Offline: write a recovery plan and its sector payloads; no device I/O.
pub fn plan_offline(
    context: &Context,
    baseline: &Path,
    current: &Path,
    output: &Path,
) -> Result<Outcome> {
    let baseline = context.baseline(baseline)?;
    let current = fs::read(current)?;
    let plan = recovery::plan(&baseline, &current)?;
    let files = recovery::payload_files(&baseline, &plan)?;
    let directory = context.root.output_directory(output)?;

    for (name, data) in files {
        evidence::write_new(&directory.join(name), &data)?;
    }

    // Through `Value`, so keys stay in the sorted order earlier plans had.
    let mut text = serde_json::to_string_pretty(&serde_json::to_value(&plan)?)?;
    text.push('\n');
    evidence::write_new(&directory.join("plan.json"), text.as_bytes())?;

    progress(&format!(
        "Offline plan: {} application-bank sectors; no device I/O",
        plan.changed_sector_count
    ));
    Outcome::done(&Planned {
        status: "planned",
        output: &directory,
        changed_sector_count: plan.changed_sector_count,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn session_state_keeps_its_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let state = Session::open(
            path.clone(),
            Details {
                port: "/dev/ttyUSB0".into(),
                operation: Kind::StockRecovery,
                reviewed_plan_sha256: "p".into(),
                helper_upload_attempted: false,
                snapshots: Vec::new(),
                transaction_directory: "transaction",
                physical_recovery_verified: false,
                tool: "wellfed-flash",
                transport: Transport::Emulator,
                scope: "scope",
                snapshot_sha256: None,
            },
        )
        .unwrap();

        state
            .update(|s| {
                s.details.snapshots.push(Snapshot {
                    transfer: Transfer {
                        bytes: CAPACITY,
                        packets: CAPACITY / 1024,
                        duplicate_packets: 0,
                    },
                    file: "snapshot-01.bin".into(),
                    sha256: "s".into(),
                })
            })
            .unwrap();
        let (state, saved) = state.close(directory.path(), |s| {
            s.status = Status::Verified;
            s.details.snapshot_sha256 = Some("a".into());
        });
        saved.unwrap();

        let written: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            written,
            json!({
                "status": "verified", "start_utc": state.start_utc, "port": "/dev/ttyUSB0",
                "operation": "original_stock_bank1_recovery", "reviewed_plan_sha256": "p",
                "helper_upload_attempted": false,
                "snapshots": [{"bytes": CAPACITY, "packets": CAPACITY / 1024,
                               "duplicate_packets": 0, "file": "snapshot-01.bin", "sha256": "s"}],
                "transaction_directory": "transaction", "physical_recovery_verified": false,
                "tool": "wellfed-flash", "transport": "emulator", "scope": "scope",
                "snapshot_sha256": "a", "end_utc": state.end_utc,
            })
        );
    }
}
