//! Two existing full-flash reads compared offline; no serial, network or flash writes.
//!
//! Port of `audit-flash-backup.py`, with the same checks and report fields.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use wellfed_flash::ota;

use crate::{Report, Result, digest, require};

pub const LAYOUT: &str = "firmware/plaf103-layout.json";
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUNS: usize = 128;
const IMAGE_MAGIC: [u32; 2] = [0x35393138, 0x31313738];

fn known_image(sha: &str) -> Option<&'static str> {
    match sha {
        ota::STOCK_SHA256 => Some("original-1.1.15"),
        "3cd8d0454f3eee84b6fc81fdf30999b1527e7ed2aad9e8ec3fadbd906da86503" => {
            Some("version-marker-1.1.16")
        }
        "fb5a04f3bdfe5b42555b684db176764018e2200ff6a4a784359d356ada8a1804" => {
            Some("c-probe-1.1.17")
        }
        "9af0ce801c538f7ff4b6edbe84f4d250791c5dedefbf01f0b18958b7123df6e7" => {
            Some("rust-probe-1.1.17")
        }
        _ => None,
    }
}

type Identity = (u64, u64, u64, i64, i64, i64, i64);

fn identity(m: &fs::Metadata) -> Identity {
    (
        m.dev(),
        m.ino(),
        m.size(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

/// A stable regular-file snapshot; reports keep no plaintext.
pub fn snapshot(path: &Path) -> Result<(Vec<u8>, Value, (u64, u64))> {
    let path = path.canonicalize()?;
    require(
        fs::metadata(&path)?.is_file(),
        "Input must be a regular file",
    )?;
    let mut handle = File::open(&path)?;
    let before = handle.metadata()?;
    require(
        before.is_file() && 0 < before.len() && before.len() <= MAX_BYTES,
        "Input must be a nonempty regular file no larger than 64 MiB",
    )?;
    let mut data = Vec::new();
    (&mut handle).take(MAX_BYTES + 1).read_to_end(&mut data)?;
    let after = handle.metadata()?;
    require(
        identity(&before) == identity(&after)
            && identity(&after) == identity(&fs::metadata(&path)?)
            && data.len() as u64 == before.len(),
        "Input changed during the audit",
    )?;
    let info =
        json!({"path": path.display().to_string(), "bytes": data.len(), "sha256": digest(&data)});
    Ok((data, info, (before.dev(), before.ino())))
}

pub fn differences(first: &[u8], second: &[u8]) -> Value {
    if first == second {
        return json!({"different_bytes": 0, "runs": [], "total_runs": 0, "runs_truncated": false});
    }
    let (mut count, mut total_runs) = (0usize, 0usize);
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    // A missing byte is a difference as well; partial reads cannot pass.
    let length = first.len().max(second.len());
    let shorter = first.len().min(second.len());
    for offset in 0..length {
        if offset >= shorter || first[offset] != second[offset] {
            count += 1;
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            if runs.len() < MAX_RUNS {
                runs.push(json!({"start": begin, "end_exclusive": offset}));
            }
            total_runs += 1;
        }
    }
    if let Some(begin) = start {
        if runs.len() < MAX_RUNS {
            runs.push(json!({"start": begin, "end_exclusive": length}));
        }
        total_runs += 1;
    }
    json!({"different_bytes": count, "runs_truncated": total_runs > runs.len(), "runs": runs, "total_runs": total_runs})
}

pub fn suspicious_patterns(data: &[u8]) -> Vec<&'static str> {
    let mut issues = Vec::new();
    if data.iter().all(|b| *b == 0xff) {
        issues.push("entire_read_is_erased_0xff");
    }
    if data.iter().all(|b| *b == 0) {
        issues.push("entire_read_is_zero");
    }
    // Repeated halves can indicate an address alias or an incorrectly doubled
    // read. They are not proof of either; flag them for investigation.
    if data.len().is_multiple_of(2) && data[..data.len() / 2] == data[data.len() / 2..] {
        issues.push("identical_halves_require_alias_or_content_investigation");
    }
    issues
}

fn refused(error: wellfed_flash::Error) -> crate::Error {
    crate::Error::Refused(error.to_string())
}

fn reference(path: Option<&Path>) -> Result<Option<Vec<u8>>> {
    let Some(path) = path else { return Ok(None) };
    let (data, _, _) = snapshot(path)?;
    require(
        digest(&data) == ota::STOCK_SHA256,
        "Reference is not the pinned original 1.1.15 OTA",
    )?;
    let payload = ota::container(&data).map_err(refused)?;
    ota::regions(payload).map_err(refused)?;
    Ok(Some(data))
}

fn hex_offset(value: &Value) -> Result<usize> {
    value
        .as_str()
        .and_then(|text| {
            usize::from_str_radix(text.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
        })
        .ok_or_else(|| crate::Error::Refused("Unsupported layout".into()))
}

fn banks(data: &[u8], layout: &Value, original: Option<&[u8]>) -> Result<Vec<Value>> {
    let size = layout["candidate_payload_cap_bytes"].as_u64().unwrap_or(0) as usize;
    let mut result = Vec::new();
    for (index, value) in layout["ota_bank_offsets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        let start = hex_offset(value)?;
        let end = start + size;
        let mut item = json!({"index": index, "start": start, "end_exclusive": end,
                              "stock_footprint_covered": end <= data.len(), "currently_active": "unknown"});
        if end <= data.len() {
            let payload = &data[start..end];
            let magic: Vec<u8> = IMAGE_MAGIC.iter().flat_map(|w| w.to_le_bytes()).collect();
            item["payload_sha256"] = json!(digest(payload));
            item["marker_matches_known_magic"] = json!(payload[..8] == magic[..]);
            if let Some(original) = original {
                // The updater invalidates the previous bank's first marker.
                // Restore ONLY eight bytes in RAM to identify its saved body;
                // never emit or recommend writing this normalized image.
                let normalized = [&original[32..40], &payload[8..]].concat();
                let mut container = [&original[..32], &normalized[..]].concat();
                let sum = normalized
                    .iter()
                    .fold(0u32, |s, b| s.wrapping_add(u32::from(*b)));
                container[16..20].copy_from_slice(&sum.to_le_bytes());
                item["image_after_marker_normalization"] =
                    json!(known_image(&digest(&container)).unwrap_or("unrecognized"));
                item["marker_normalization_bytes"] = json!(8);
                item["matches_original_except_marker"] = json!(normalized[..] == original[32..]);
            }
        }
        result.push(item);
    }
    Ok(result)
}

pub struct Inputs<'a> {
    pub root: &'a Path,
    pub first: &'a Path,
    pub second: &'a Path,
    pub capacity: u64,
    pub unit_label: &'a str,
    pub stock: Option<&'a Path>,
    pub capacity_evidence: Option<&'a Path>,
}

pub fn audit(inputs: &Inputs) -> Result<Report> {
    let capacity = inputs.capacity;
    require(
        0 < capacity && capacity <= MAX_BYTES && capacity.is_multiple_of(4096),
        "Capacity must be explicit, positive, sector aligned and no larger than 64 MiB",
    )?;
    let label = inputs.unit_label;
    require(
        (1..=64).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
        "Use a short non-secret unit label, not credentials or a MAC address",
    )?;
    let layout_path = inputs.root.join(LAYOUT);
    let layout_raw = fs::read(&layout_path)?;
    let layout: Value = serde_json::from_slice(&layout_raw)?;
    require(
        layout["schema_version"] == 1
            && layout["sector_bytes"] == 4096
            && layout["stock_sha256"] == ota::STOCK_SHA256,
        "Unsupported layout",
    )?;
    let (first, first_info, first_identity) = snapshot(inputs.first)?;
    let (second, second_info, second_identity) = snapshot(inputs.second)?;
    require(
        first_identity != second_identity,
        "Two paths to the same file are not separate reads",
    )?;
    let original = reference(inputs.stock)?;
    let evidence = match inputs.capacity_evidence {
        Some(path) => snapshot(path)?.1,
        None => Value::Null,
    };
    let diff = differences(&first, &second);
    let lengths_match = first.len() as u64 == capacity && second.len() as u64 == capacity;
    let anomalies =
        json!({"first": suspicious_patterns(&first), "second": suspicious_patterns(&second)});
    let any_anomaly =
        !suspicious_patterns(&first).is_empty() || !suspicious_patterns(&second).is_empty();
    let mut preserve = Vec::new();
    for area in layout["known_preserve_ranges"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let (start, end) = (
            hex_offset(&area["start"])?,
            hex_offset(&area["end_exclusive"])?,
        );
        let covered = end <= first.len().min(second.len());
        let mut entry = json!({"name": area["name"], "start": start, "end_exclusive": end, "covered_in_both": covered});
        if covered {
            let (a, b) = (&first[start..end], &second[start..end]);
            entry["first_sha256"] = json!(digest(a));
            entry["second_sha256"] = json!(digest(b));
            entry["identical"] = json!(a == b);
            entry["different_bytes"] = json!(a.iter().zip(b).filter(|(x, y)| x != y).count());
        }
        preserve.push(entry);
    }
    let mut changed_sectors = Vec::new();
    for start in (0..first.len().max(second.len())).step_by(4096) {
        let slice = |d: &[u8]| {
            d.get(start..(start + 4096).min(d.len()))
                .unwrap_or(&[])
                .to_vec()
        };
        if slice(&first) != slice(&second) {
            let areas: Vec<&Value> = preserve
                .iter()
                .filter(|r| {
                    start < r["end_exclusive"].as_u64().unwrap_or(0) as usize
                        && (r["start"].as_u64().unwrap_or(0) as usize) < start + 4096
                })
                .map(|r| &r["name"])
                .collect();
            changed_sectors.push(json!({"start": start, "known_preserve_areas": areas}));
        }
    }
    let all_covered = preserve.iter().all(|r| r["covered_in_both"] == true);
    let identical = diff["different_bytes"] == 0;
    let passed = lengths_match && identical && !any_anomaly && all_covered;
    let report = json!({
        "schema_version": 1, "created_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string(),
        "unit_label": label, "operator_supplied_capacity_bytes": capacity,
        "capacity_evidence": evidence, "capacity_evidence_interpreted": false,
        "inputs": [first_info, second_info],
        "checks": {"lengths_match_supplied_capacity": lengths_match, "byte_identical": identical,
                   "no_detected_pattern_anomalies": !any_anomaly, "all_observed_preserve_areas_covered": all_covered},
        "comparison_passed": passed, "differences": diff, "changed_sectors": changed_sectors,
        "pattern_anomalies": anomalies, "known_preserve_areas": preserve,
        "application_banks": {"first": banks(&first, &layout, original.as_deref())?,
                              "second": banks(&second, &layout, original.as_deref())?},
        "stock_reference_sha256": original.as_deref().map(digest),
        "limits": {"same_physical_unit_verified": false, "independent_acquisitions_verified": false,
                   "physical_capacity_verified": false, "read_method_verified_non_erasing": false,
                   "complete_partition_map": false, "failed_boot_recovery_verified": false,
                   "boot_selection_verified": false, "restorable_backup_verified": false},
        "scope": "Offline comparison of supplied raw reads, not authorization or a procedure to flash or clone a unit",
        "tool": "wellfed-audit backup",
        "tool_sha256": std::env::current_exe().ok().and_then(|exe| fs::read(exe).ok()).map(|d| digest(&d)),
        "layout_sha256": digest(&layout_raw),
    });
    Ok(match report {
        Value::Object(map) => map,
        _ => unreachable!(),
    })
}

/// A new private report: never an overwrite, a symlink target or an input.
pub fn write_report(
    root: &Path,
    path: &Path,
    report: &Report,
    inputs: &[&Path],
) -> Result<PathBuf> {
    require(
        fs::symlink_metadata(path).is_err(),
        "Report already exists; choose a new path",
    )?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let resolved = parent
        .canonicalize()?
        .join(path.file_name().unwrap_or_default());
    require(
        resolved.starts_with(root.join("private").canonicalize()?),
        "Report must be under ignored private/",
    )?;
    require(
        !inputs
            .iter()
            .any(|input| input.canonicalize().is_ok_and(|p| p == resolved)),
        "Report cannot overwrite an input",
    )?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&resolved)?;
    file.write_all(format!("{}\n", serde_json::to_string_pretty(report)?).as_bytes())?;
    file.sync_all()?;
    Ok(resolved)
}

#[cfg(test)]
mod tests;
