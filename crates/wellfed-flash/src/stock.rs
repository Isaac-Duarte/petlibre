//! Prepare or execute exact original bank-1 recovery on the research unit.
//!
//! Port of the `prepare`/`execute` modes of `tools/flash/uart-stock-recovery.py`.
//! It continues an established pinned helper session from a complete full
//! read; it never uploads, changes status, enters a loader or boots. An
//! invalid target marker is required for a changed-bank recovery. The Python
//! client's write-back acceptance-test modes are not ported.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::canonical::{canonical, sha256};
use crate::error::{Result, require};
use crate::evidence::{self, utc};
use crate::protocol::{self, CAPACITY, HELPER_SHA256, Port};
use crate::recovery::{self, KIND};
use crate::session::{self, Context, Outcome, progress};
use crate::update::Evidence;

pub struct Args {
    pub execute: bool,
    pub evidence: Evidence,
    /// A complete full read from this helper session, beside its report.
    pub current: PathBuf,
}

/// All local provenance checks run before serial access or output creation.
fn prepare_inputs(
    context: &Context,
    args: &Args,
) -> Result<(crate::sector::Baseline, Vec<u8>, Value)> {
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
    let report: Value = serde_json::from_slice(&raw_report)?;
    context.same_transport(&report)?;

    let result = &report["result"];
    require(
        report["mode"] == "read-full"
            && report["status"] == "complete"
            && report["port"] == evidence.port.as_str()
            && report["helper_sha256"] == HELPER_SHA256
            && report["helper_upload_attempted"] == false
            && report["bounded_read_report_sha256"] == sha256(&small).as_str()
            && result["offset"].as_u64() == Some(0)
            && result["bytes"].as_u64() == Some(CAPACITY as u64)
            && result["sha256"] == sha256(&current).as_str(),
        "Complete full-read report from this helper session required",
    )?;
    session::streams_match(
        root,
        &report_path,
        &report,
        "Full-read serial evidence hash mismatch",
    )?;

    let reviewed = json!({
        "schema_version": 1, "kind": KIND, "port": evidence.port, "recovery_plan": plan,
        "current_report_sha256": sha256(&raw_report),
        "bounded_read_report_sha256": sha256(&small), "rom_report_sha256": sha256(&rom),
        "tool_sha256": context.tools(),
    });

    Ok((baseline, current, reviewed))
}

/// Everything but the nested planner's creation time, never a guard.
fn comparable(reviewed: &Value) -> Result<Vec<u8>> {
    let mut result = reviewed.clone();
    require(
        result["recovery_plan"]["created_utc"].is_string(),
        "Plan creation time required",
    )?;

    result["recovery_plan"]
        .as_object_mut()
        .unwrap()
        .remove("created_utc");
    canonical(&result)
}

pub fn run(context: &Context, args: &Args) -> Result<Outcome> {
    let (baseline, current, expected) = prepare_inputs(context, args)?;
    let evidence = &args.evidence;

    if !args.execute {
        let sha = session::write_plan(&context.root, &evidence.output, &expected)?;
        progress(&format!("Prepared bank-1 recovery plan SHA256: {sha}"));
        return Ok(Outcome {
            complete: true,
            summary: json!({"status": "prepared", "operation": KIND, "plan": evidence.output,
                            "plan_sha256": sha,
                            "changed_sectors": expected["recovery_plan"]["changed_sector_count"]}),
        });
    }

    let (Some(plan), Some(expected_sha)) = (&evidence.plan, &evidence.expected_plan_sha256) else {
        return Err(crate::Error::Refused(
            "Exact reviewed plan/hash required".into(),
        ));
    };

    let raw = context.root.private_file(plan)?;
    require(
        sha256(&raw) == *expected_sha,
        "Reviewed plan SHA256 differs",
    )?;

    let reviewed: Value = serde_json::from_slice(&raw)?;
    require(
        comparable(&reviewed)? == comparable(&expected)?,
        "Reviewed plan/source/evidence differs",
    )?;

    recovery::validate_plan(&baseline, &current, &reviewed["recovery_plan"])?;
    execute(context, args, &baseline, &current, &reviewed, &raw)
}

/// Open once; the transaction owns durable mutation intent and activation order.
fn execute(
    context: &Context,
    args: &Args,
    baseline: &crate::sector::Baseline,
    current: &[u8],
    reviewed: &Value,
    raw: &[u8],
) -> Result<Outcome> {
    let directory = context.root.output_directory(&args.evidence.output)?;
    session::keep_plan(&directory, raw)?;
    if let Some(parent) = directory.parent() {
        evidence::sync_directory(parent)?;
    }

    let path = directory.join("session.json");
    let state = RefCell::new(json!({
        "status": "prepared", "start_utc": utc(), "port": args.evidence.port, "operation": KIND,
        "reviewed_plan_sha256": sha256(raw), "helper_upload_attempted": false,
        "snapshots": [], "transaction_directory": "transaction",
        "physical_recovery_verified": false, "tool": "wellfed-flash", "transport": context.transport(),
        "scope": "Supervised serial stock restore; physical acceptance requires external bench evidence",
    }));
    let save = || evidence::save_report(&path, &state.borrow());
    save()?;

    let result = (|| -> Result<Value> {
        let mut port = context.record(&args.evidence.port, &directory)?;
        state.borrow_mut()["status"] = json!("running");
        save()?;

        progress("Original bank-1 operation; fresh full verification before any mutation");

        let mut snapshot = |port: &mut dyn Port| -> Result<Vec<u8>> {
            let number = state.borrow()["snapshots"].as_array().unwrap().len() + 1;
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

            let mut entry = Value::Object(transfer.json());
            entry["file"] = json!(name);
            entry["sha256"] = json!(sha256(&data));

            state.borrow_mut()["snapshots"]
                .as_array_mut()
                .unwrap()
                .push(entry);
            save()?;
            Ok(data)
        };

        recovery::run_transaction(
            &mut port,
            &context.root,
            baseline,
            current,
            &reviewed["recovery_plan"],
            &mut snapshot,
            &directory.join("transaction"),
        )
    })();

    let complete = result.is_ok();
    {
        let mut state = state.borrow_mut();
        match result {
            Ok(report) => {
                state["status"] = json!("verified");
                state["snapshot_sha256"] = report["snapshot_sha256"].clone();
                progress(
                    "Bank-1 recovery and preserved ranges verified; keep application boot excluded until review",
                );
            }
            Err(error) => {
                state["status"] = json!("stopped");
                state["error"] = json!(format!("{}: {error}", error.kind()));
                progress(
                    "Stopped without replay. Keep application boot excluded and review private evidence.",
                );
            }
        }
        state["end_utc"] = json!(utc());
        evidence::stream_hashes(&directory, &mut state);
    }

    let saved = save();
    let state = state.into_inner();
    Ok(session::closed(
        complete,
        json!({"status": state["status"], "operation": KIND, "output": directory,
               "snapshot_sha256": state.get("snapshot_sha256"), "error": state.get("error")}),
        saved,
    ))
}

/// Offline: write a recovery plan and its sector payloads; no device I/O.
pub fn plan_offline(
    context: &Context,
    baseline: &std::path::Path,
    current: &std::path::Path,
    output: &std::path::Path,
) -> Result<Value> {
    let baseline = context.baseline(baseline)?;
    let current = fs::read(current)?;
    let plan = recovery::plan(&baseline, &current)?;
    let files = recovery::payload_files(&baseline, &plan)?;
    let directory = context.root.output_directory(output)?;

    for (name, data) in files {
        evidence::write_new(&directory.join(name), &data)?;
    }

    let mut text = serde_json::to_string_pretty(&plan)?;
    text.push('\n');
    evidence::write_new(&directory.join("plan.json"), text.as_bytes())?;

    progress(&format!(
        "Offline plan: {} application-bank sectors; no device I/O",
        plan["changed_sector_count"]
    ));
    Ok(json!({"status": "planned", "output": directory,
              "changed_sector_count": plan["changed_sector_count"]}))
}
