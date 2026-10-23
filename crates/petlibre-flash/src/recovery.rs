//! Sector-scoped original-stock recovery of bank 1.
//!
//! Port of `tools/flash/plan-stock-recovery.py` and `run_transaction` in
//! `tools/flash/recovery-transaction.py`. Every byte outside bank 1 is
//! preserved from the current snapshot; the marker sector is staged last with
//! eight erased bytes and committed in a separate no-erase pass after full
//! verification.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Result, refuse, require};
use crate::evidence::{Root, utc};
use crate::journal::{Event, Journal};
use crate::protocol::{CAPACITY, Port, SECTOR};
use crate::sector::{self, Baseline, END, MarkerCommit, Restored, START};
use petlibre_formats::canonical::{canonical, sha256};

/// The plan `plan-stock-recovery.py` writes, field for field; reviewed
/// plans bind its canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub schema_version: u32,
    pub created_utc: String,
    pub baseline_sha256: String,
    pub current_snapshot_sha256: String,
    pub proposed_snapshot_sha256: String,
    pub target_bank: u8,
    pub target_range: Range,
    pub sector_bytes: usize,
    pub changed_sector_count: usize,
    pub ordered_sector_payloads: Vec<SectorPayload>,
    /// `null` when no sector changes.
    pub activation: Option<Activation>,
    pub preserve_current_ranges: Vec<Preserved>,
    pub scope: &'static str,
    pub required_before_any_write: [&'static str; 9],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Range {
    pub start: usize,
    pub end_exclusive: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectorPayload {
    pub offset: usize,
    pub bytes: usize,
    pub before_sha256: String,
    pub after_sha256: String,
    pub stage_sha256: String,
    pub stage_required: bool,
    pub marker_withheld: bool,
    /// The payload file `plan-stock-recovery` writes beside the plan.
    pub payload: String,
}

/// The separate no-erase marker commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activation {
    pub marker_offset: usize,
    pub marker_bytes: usize,
    pub additional_erase: bool,
    pub staged_snapshot_sha256: String,
    pub payload: &'static str,
    pub payload_bytes: usize,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preserved {
    pub start: usize,
    pub end_exclusive: usize,
    pub sha256: String,
}

const REQUIRED_BEFORE_ANY_WRITE: [&str; 9] = [
    "Operator must establish current snapshot is from the research feeder; hashes do not identify hardware",
    "Prove independent hardware loader entry and obtain stable current readback before stock can run",
    "Review pinned helper write/erase protocol and accepted protection/encryption behavior",
    "Use only sector erase; never chip erase or blanket protection/status-register clearing",
    "Check each sector against before_sha256 immediately before erasing; stop on mismatch",
    "Read back staged sectors against stage_sha256, withholding the first eight marker bytes",
    "Verify staged full image and current preserved ranges before a separate no-erase marker commit",
    "Verify full target bank and all preserved current ranges before attempting stock boot",
    "No blind retries, automatic fallback writes or writes to factory/OTP/eFuse/settings",
];

/// The offline recovery plan: no executable command, only sector payload
/// hashes and the preconditions every write must meet.
pub fn plan(baseline: &Baseline, current: &[u8]) -> Result<Plan> {
    let baseline = baseline.bytes();
    require(current.len() == CAPACITY, "Need two exact 8 MiB snapshots")?;
    require(
        current[..0x6000] == baseline[..0x6000],
        "Factory/pre-application bytes differ; stop for investigation",
    )?;

    let mut sectors = Vec::new();
    let mut proposed = current.to_vec();

    for offset in (START..END).step_by(SECTOR) {
        let (before, after) = (
            &current[offset..offset + SECTOR],
            &baseline[offset..offset + SECTOR],
        );
        if before == after {
            continue;
        }

        let mut staged = after.to_vec();
        if offset == START {
            staged[..8].fill(0xff);
        }

        sectors.push(SectorPayload {
            offset,
            bytes: SECTOR,
            before_sha256: sha256(before),
            after_sha256: sha256(after),
            stage_sha256: sha256(&staged),
            stage_required: before != staged,
            marker_withheld: offset == START,
            payload: format!("sector-{offset:06x}.bin"),
        });
        proposed[offset..offset + SECTOR].copy_from_slice(after);
    }

    require(
        proposed[..START] == current[..START] && proposed[END..] == current[END..],
        "Proposed bytes escape application bank 1",
    )?;
    require(
        proposed[START..END] == baseline[START..END],
        "Proposed bank is not exact baseline stock",
    )?;
    require(
        sectors.is_empty() || current[START..START + 8] != baseline[START..START + 8],
        "Target marker is still valid; refuse an in-place rewrite of a bootable bank",
    )?;

    // The marker sector goes last; activation is a separate no-erase pass.
    sectors.sort_by_key(|entry| (entry.offset == START, entry.offset));

    let preserved = [(0, START), (END, CAPACITY)]
        .into_iter()
        .map(|(start, end_exclusive)| Preserved {
            start,
            end_exclusive,
            sha256: sha256(&current[start..end_exclusive]),
        })
        .collect();

    let activation = (!sectors.is_empty()).then(|| {
        let mut staged = proposed.clone();
        staged[START..START + 8].fill(0xff);
        Activation {
            marker_offset: START,
            marker_bytes: 8,
            additional_erase: false,
            staged_snapshot_sha256: sha256(&staged),
            payload: "marker-commit-1k.bin",
            payload_bytes: 1024,
            payload_sha256: sha256(&baseline[START..START + 1024]),
        }
    });

    Ok(Plan {
        schema_version: 2,
        created_utc: utc(),
        baseline_sha256: sha256(baseline),
        current_snapshot_sha256: sha256(current),
        proposed_snapshot_sha256: sha256(&proposed),
        target_bank: 1,
        target_range: Range {
            start: START,
            end_exclusive: END,
        },
        sector_bytes: SECTOR,
        changed_sector_count: sectors.len(),
        ordered_sector_payloads: sectors,
        activation,
        preserve_current_ranges: preserved,
        scope: "Offline plan only; no physical operation authorized or executed by this tool",
        required_before_any_write: REQUIRED_BEFORE_ANY_WRITE,
    })
}

/// A plan without its creation time, which is the only field allowed to differ.
pub fn without_time(plan: &Value) -> Result<Vec<u8>> {
    let mut plan = plan.clone();
    if let Some(map) = plan.as_object_mut() {
        map.remove("created_utc");
    }
    Ok(canonical(&plan)?)
}

/// The recomputed plan, once `manifest` (as reviewed, from either writer)
/// matches it in everything but its creation time.
pub fn validate_plan(baseline: &Baseline, current: &[u8], manifest: &Value) -> Result<Plan> {
    let expected = plan(baseline, current)?;
    require(
        manifest.get("created_utc").is_some_and(Value::is_string),
        "Reviewed plan required",
    )?;
    require(
        without_time(manifest)? == without_time(&serde_json::to_value(&expected)?)?,
        "Plan differs from recomputed exact baseline/current plan",
    )?;
    Ok(expected)
}

/// The offline payload files of a plan.
pub fn payload_files(baseline: &Baseline, plan: &Plan) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    for entry in &plan.ordered_sector_payloads {
        let mut payload = baseline.bytes()[entry.offset..entry.offset + SECTOR].to_vec();
        if entry.marker_withheld {
            payload[..8].fill(0xff);
        }
        require(
            entry.stage_sha256 == sha256(&payload),
            "Staging payload hash differs",
        )?;
        files.push((entry.payload.clone(), payload));
    }

    if let Some(activation) = &plan.activation {
        files.push((
            activation.payload.to_string(),
            baseline.bytes()[START..START + 1024].to_vec(),
        ));
    }

    Ok(files)
}

pub type Snapshot<'a> = dyn FnMut(&mut dyn Port) -> Result<Vec<u8>> + 'a;

/// The report a verified recovery saves beside its journal.
#[derive(Debug, Clone, Serialize)]
pub struct Recovered {
    pub status: &'static str,
    pub transaction_id: String,
    pub sectors: Vec<Restored>,
    /// `null` for a plan with nothing to restore.
    pub marker_commit: Option<MarkerCommit>,
    pub snapshot_sha256: String,
    pub physical_recovery_verified: bool,
    pub same_unit_identity_verified_by_component: bool,
    pub scope: &'static str,
}

/// Execute a reviewed recovery plan; the journal goes to `output`.
///
/// `manifest` stays the reviewed JSON: the journal binds the hash of exactly
/// what was reviewed, whichever tool wrote it.
pub fn run_transaction(
    port: &mut dyn Port,
    root: &Root,
    baseline: &Baseline,
    current: &[u8],
    manifest: &Value,
    snapshot: &mut Snapshot,
    output: &Path,
) -> Result<Recovered> {
    let expected = validate_plan(baseline, current, manifest)?;

    // A previous output is terminal; there is no resume path.
    let mut journal = Journal::create(root, output)?;
    let transaction = uuid::Uuid::new_v4().to_string();
    let last = std::cell::RefCell::new(None::<Vec<u8>>);

    let mut full = |port: &mut dyn Port| -> Result<Vec<u8>> {
        let value = snapshot(port)?;
        require(
            value.len() == CAPACITY,
            "Snapshot provider must return exactly 8 MiB",
        )?;
        *last.borrow_mut() = Some(value.clone());
        Ok(value)
    };

    let result = (|| -> Result<Recovered> {
        journal.append(&Event::RecoveryStart {
            transaction_id: transaction.clone(),
            baseline_sha256: baseline.sha256(),
            current_sha256: sha256(current),
            plan_sha256: sha256(&canonical(manifest)?),
        })?;

        require(
            full(port)? == current,
            "Fresh full snapshot differs from reviewed current image",
        )?;
        journal.append(&Event::InitialSnapshotVerified {
            sha256: sha256(current),
        })?;

        let mut results = Vec::new();
        for entry in &expected.ordered_sector_payloads {
            let mut append = |event: crate::journal::At| journal.append(&event);
            let result = sector::restore_sector(
                port,
                baseline,
                entry.offset,
                &entry.before_sha256,
                &mut append,
            )?;

            let packets = if entry.stage_required { 4 } else { 0 };
            require(
                result.after_sha256 == entry.stage_sha256 && result.program_packets == packets,
                "Sector result differs from reviewed staging plan",
            )?;
            results.push(result);
        }

        let mut marker = None;
        if let Some(last_result) = results.last() {
            require(
                last_result.offset == START && last_result.marker_withheld,
                "Marker sector was not staged last",
            )?;
            let mut append = |event: crate::journal::At| journal.append(&event);
            marker = Some(sector::commit_marker(
                port,
                baseline,
                current,
                &mut full,
                &mut append,
            )?);
        }

        // The commit's last action verified a full final snapshot, and a no-op
        // plan performs nothing after the initial one; reuse it.
        let Some(after) = last.borrow().clone() else {
            return refuse("No final snapshot was taken");
        };
        require(
            after[..START] == current[..START] && after[END..] == current[END..],
            "Final preserved ranges differ from current snapshot",
        )?;
        require(
            after[START..END] == baseline.bytes()[START..END]
                && sha256(&after) == expected.proposed_snapshot_sha256,
            "Final stock bank/full-image verification failed",
        )?;

        journal.append(&Event::RecoveryVerified {
            snapshot_sha256: sha256(&after),
            sector_count: results.len(),
            marker_commit: marker.is_some(),
        })?;

        let report = Recovered {
            status: "verified_via_supplied_transports",
            transaction_id: transaction.clone(),
            sectors: results,
            marker_commit: marker,
            snapshot_sha256: sha256(&after),
            physical_recovery_verified: false,
            same_unit_identity_verified_by_component: false,
            scope: "Transport/snapshot callback checks only; physical evidence remains external",
        };
        journal.save_report(&report)?;
        Ok(report)
    })();

    if let Err(error) = &result {
        journal.stopped(error, Some(&transaction));
    }
    result
}
