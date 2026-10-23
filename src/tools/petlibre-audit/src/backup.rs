//! Two existing full-flash reads compared offline; no serial, network or flash writes.
//!
//! Port of `audit-flash-backup.py`, with the same checks and report fields.
//!
//! Both reads are stable snapshots of distinct files. The comparison passes
//! only when each is exactly the operator-supplied capacity, they are
//! byte-identical, neither is all 0xFF, all zero or two identical halves, and
//! both cover every known preserve range in the layout. An optional pinned
//! stock OTA lets each application bank's body be named. A pass is still
//! only agreement between two files: the report's `limits` record that
//! capacity, independent acquisition and restorability stay unverified.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use petlibre_flash::ota;
use petlibre_formats::layout::{Hex, Layout};
use serde::Serialize;

use crate::{Error, Result, digest, require};

pub(crate) const LAYOUT: &str = "src/firmware/vendor/plaf103-layout.json";
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUNS: usize = 128;
/// The bank marker `81958711` as two little-endian words; a valid bank's image
/// starts with it (docs/firmware/bank-policy.md).
const IMAGE_MAGIC: [u32; 2] = [0x35393138, 0x31313738];

/// Names a bank's saved body from its OTA container hash.
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

/// Device, inode, size and both timestamps; any change while the file is read
/// means it was not a stable snapshot.
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

/// An input as the report records it: by hash, never its content.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileInfo {
    path: String,
    bytes: usize,
    sha256: String,
}

/// A stable regular-file snapshot: its bytes, its record and its device
/// and inode, which tell two paths to the same file apart from two reads.
fn snapshot(path: &Path) -> Result<(Vec<u8>, FileInfo, (u64, u64))> {
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

    let info = FileInfo {
        path: path.display().to_string(),
        bytes: data.len(),
        sha256: digest(&data),
    };
    Ok((data, info, (before.dev(), before.ino())))
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Run {
    start: usize,
    end_exclusive: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Differences {
    different_bytes: usize,
    runs_truncated: bool,
    /// The first `MAX_RUNS` runs.
    runs: Vec<Run>,
    total_runs: usize,
}

/// Differing bytes and their runs; only the first `MAX_RUNS` runs are listed,
/// but every run is counted.
fn differences(first: &[u8], second: &[u8]) -> Differences {
    let (mut count, mut total_runs) = (0usize, 0usize);
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    let mut close = |begin, end_exclusive, runs: &mut Vec<Run>| {
        if runs.len() < MAX_RUNS {
            runs.push(Run {
                start: begin,
                end_exclusive,
            });
        }
        total_runs += 1;
    };

    // A missing byte is a difference as well; partial reads cannot pass.
    let length = first.len().max(second.len());
    let shorter = first.len().min(second.len());
    if first != second {
        for offset in 0..length {
            if offset >= shorter || first[offset] != second[offset] {
                count += 1;
                start.get_or_insert(offset);
            } else if let Some(begin) = start.take() {
                close(begin, offset, &mut runs);
            }
        }
        if let Some(begin) = start {
            close(begin, length, &mut runs);
        }
    }

    Differences {
        different_bytes: count,
        runs_truncated: total_runs > runs.len(),
        runs,
        total_runs,
    }
}

fn suspicious_patterns(data: &[u8]) -> Vec<&'static str> {
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

fn reference(path: Option<&Path>) -> Result<Option<Vec<u8>>> {
    let Some(path) = path else { return Ok(None) };
    let (data, _, _) = snapshot(path)?;
    require(
        digest(&data) == ota::STOCK_SHA256,
        "Reference is not the pinned original 1.1.15 OTA",
    )?;

    let payload = ota::container(&data)?;
    ota::images(payload)?;
    Ok(Some(data))
}

/// A layout offset as an index into a read.
fn offset(value: Hex) -> Result<usize> {
    usize::try_from(value.0).map_err(|_| Error::Refused("Unsupported layout".into()))
}

/// One application bank's footprint in a read.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Bank {
    index: usize,
    start: usize,
    end_exclusive: usize,
    stock_footprint_covered: bool,
    /// Which bank boots is not established by a read.
    currently_active: &'static str,
    /// Only when the read covers the footprint.
    #[serde(flatten)]
    payload: Option<BankPayload>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BankPayload {
    payload_sha256: String,
    marker_matches_known_magic: bool,
    /// Only with the pinned stock reference.
    #[serde(flatten)]
    normalized: Option<Normalized>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Normalized {
    image_after_marker_normalization: &'static str,
    marker_normalization_bytes: usize,
    matches_original_except_marker: bool,
}

/// Name a bank's saved body against the pinned original.
fn normalize(payload: &[u8], original: &[u8]) -> Normalized {
    // The updater invalidates the previous bank's first marker. Restore ONLY
    // eight bytes in RAM to identify its saved body; never emit or recommend
    // writing this normalized image.
    let normalized = [&original[32..40], &payload[8..]].concat();
    let mut container = [&original[..32], &normalized[..]].concat();
    let sum = normalized
        .iter()
        .fold(0u32, |s, b| s.wrapping_add(u32::from(*b)));
    container[16..20].copy_from_slice(&sum.to_le_bytes());

    Normalized {
        image_after_marker_normalization: known_image(&digest(&container))
            .unwrap_or("unrecognized"),
        marker_normalization_bytes: 8,
        matches_original_except_marker: normalized[..] == original[32..],
    }
}

fn banks(data: &[u8], layout: &Layout, original: Option<&[u8]>) -> Result<Vec<Bank>> {
    let size = offset(Hex(layout.candidate_payload_cap_bytes))?;
    let magic: Vec<u8> = IMAGE_MAGIC.iter().flat_map(|w| w.to_le_bytes()).collect();

    let mut result = Vec::new();
    for (index, &value) in layout.ota_bank_offsets.iter().enumerate() {
        let start = offset(value)?;
        let end = start + size;
        let payload = data.get(start..end).map(|payload| BankPayload {
            payload_sha256: digest(payload),
            marker_matches_known_magic: payload[..8] == magic[..],
            normalized: original.map(|original| normalize(payload, original)),
        });

        result.push(Bank {
            index,
            start,
            end_exclusive: end,
            stock_footprint_covered: end <= data.len(),
            currently_active: "unknown",
            payload,
        });
    }

    Ok(result)
}

/// What the operator supplied. Capacity is never inferred from the reads
/// themselves, since a short or aliased read would then confirm itself.
pub struct Inputs<'a> {
    pub root: &'a Path,
    pub first: &'a Path,
    pub second: &'a Path,
    pub capacity: u64,
    pub unit_label: &'a str,
    pub stock: Option<&'a Path>,
    pub capacity_evidence: Option<&'a Path>,
}

/// One known preserve range in both reads.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PreserveArea {
    name: String,
    start: usize,
    end_exclusive: usize,
    covered_in_both: bool,
    /// Only when both reads cover the range.
    #[serde(flatten)]
    comparison: Option<AreaComparison>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AreaComparison {
    first_sha256: String,
    second_sha256: String,
    identical: bool,
    different_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChangedSector {
    start: usize,
    known_preserve_areas: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Checks {
    lengths_match_supplied_capacity: bool,
    byte_identical: bool,
    no_detected_pattern_anomalies: bool,
    all_observed_preserve_areas_covered: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PerRead<T> {
    first: T,
    second: T,
}

/// What a comparison never establishes; every field is written `false`.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Limits {
    same_physical_unit_verified: bool,
    independent_acquisitions_verified: bool,
    physical_capacity_verified: bool,
    read_method_verified_non_erasing: bool,
    complete_partition_map: bool,
    failed_boot_recovery_verified: bool,
    boot_selection_verified: bool,
    restorable_backup_verified: bool,
}

/// The backup audit's report.
#[derive(Debug, Clone, Serialize)]
pub struct BackupReport {
    schema_version: u32,
    created_at: String,
    unit_label: String,
    operator_supplied_capacity_bytes: u64,
    capacity_evidence: Option<FileInfo>,
    capacity_evidence_interpreted: bool,
    inputs: [FileInfo; 2],
    checks: Checks,
    comparison_passed: bool,
    differences: Differences,
    changed_sectors: Vec<ChangedSector>,
    pattern_anomalies: PerRead<Vec<&'static str>>,
    known_preserve_areas: Vec<PreserveArea>,
    application_banks: PerRead<Vec<Bank>>,
    stock_reference_sha256: Option<String>,
    limits: Limits,
    scope: &'static str,
    tool: &'static str,
    tool_sha256: Option<String>,
    layout_sha256: String,
}

impl BackupReport {
    pub fn passed(&self) -> bool {
        self.comparison_passed
    }
}

fn preserve_areas(layout: &Layout, first: &[u8], second: &[u8]) -> Result<Vec<PreserveArea>> {
    let mut areas = Vec::new();
    for area in &layout.known_preserve_ranges {
        let (start, end) = (offset(area.start)?, offset(area.end_exclusive)?);
        let covered = end <= first.len().min(second.len());
        let comparison = covered.then(|| {
            let (a, b) = (&first[start..end], &second[start..end]);
            AreaComparison {
                first_sha256: digest(a),
                second_sha256: digest(b),
                identical: a == b,
                different_bytes: a.iter().zip(b).filter(|(x, y)| x != y).count(),
            }
        });

        areas.push(PreserveArea {
            name: area.name.clone(),
            start,
            end_exclusive: end,
            covered_in_both: covered,
            comparison,
        });
    }
    Ok(areas)
}

/// Every differing 4 KiB sector, with the known areas it overlaps.
fn changed_sectors(layout: &Layout, first: &[u8], second: &[u8]) -> Vec<ChangedSector> {
    let slice = |d: &[u8], start: usize| -> Vec<u8> {
        d.get(start..(start + 4096).min(d.len()))
            .unwrap_or(&[])
            .to_vec()
    };

    (0..first.len().max(second.len()))
        .step_by(4096)
        .filter(|&start| slice(first, start) != slice(second, start))
        .map(|start| {
            let sector = start as u64..start as u64 + 4096;
            let areas = layout
                .known_preserve_ranges
                .iter()
                .filter(|r| sector.start < r.end_exclusive.0 && r.start.0 < sector.end)
                .map(|r| r.name.clone())
                .collect();
            ChangedSector {
                start,
                known_preserve_areas: areas,
            }
        })
        .collect()
}

pub fn audit(inputs: &Inputs) -> Result<BackupReport> {
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
    let layout: Layout = serde_json::from_slice(&layout_raw)?;
    require(
        layout.schema_version == 1
            && layout.sector_bytes == 4096
            && layout.stock_sha256 == ota::STOCK_SHA256,
        "Unsupported layout",
    )?;

    let (first, first_info, first_identity) = snapshot(inputs.first)?;
    let (second, second_info, second_identity) = snapshot(inputs.second)?;
    require(
        first_identity != second_identity,
        "Two paths to the same file are not separate reads",
    )?;

    let original = reference(inputs.stock)?;
    let evidence = inputs
        .capacity_evidence
        .map(|path| Ok::<_, Error>(snapshot(path)?.1))
        .transpose()?;

    let differences = differences(&first, &second);
    let anomalies = PerRead {
        first: suspicious_patterns(&first),
        second: suspicious_patterns(&second),
    };
    let preserve = preserve_areas(&layout, &first, &second)?;
    let checks = Checks {
        lengths_match_supplied_capacity: first.len() as u64 == capacity
            && second.len() as u64 == capacity,
        byte_identical: differences.different_bytes == 0,
        no_detected_pattern_anomalies: anomalies.first.is_empty() && anomalies.second.is_empty(),
        all_observed_preserve_areas_covered: preserve.iter().all(|r| r.covered_in_both),
    };
    let passed = checks.lengths_match_supplied_capacity
        && checks.byte_identical
        && checks.no_detected_pattern_anomalies
        && checks.all_observed_preserve_areas_covered;

    Ok(BackupReport {
        schema_version: 1,
        created_at: chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.6f+00:00")
            .to_string(),
        unit_label: label.into(),
        operator_supplied_capacity_bytes: capacity,
        capacity_evidence: evidence,
        capacity_evidence_interpreted: false,
        inputs: [first_info, second_info],
        checks,
        comparison_passed: passed,
        changed_sectors: changed_sectors(&layout, &first, &second),
        differences,
        pattern_anomalies: anomalies,
        known_preserve_areas: preserve,
        application_banks: PerRead {
            first: banks(&first, &layout, original.as_deref())?,
            second: banks(&second, &layout, original.as_deref())?,
        },
        stock_reference_sha256: original.as_deref().map(digest),
        limits: Limits::default(),
        scope: "Offline comparison of supplied raw reads, not authorization or a procedure to flash or clone a unit",
        tool: "petlibre-audit backup",
        tool_sha256: std::env::current_exe()
            .ok()
            .and_then(|exe| fs::read(exe).ok())
            .map(|d| digest(&d)),
        layout_sha256: digest(&layout_raw),
    })
}

#[cfg(test)]
mod tests;
