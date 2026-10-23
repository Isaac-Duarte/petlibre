//! Offline raw-read comparison; all public fixtures are synthetic.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::write_report;

/// Test fixture only, never a hardware-capacity assertion.
const CAPACITY: u64 = 8 * 1024 * 1024;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn synthetic() -> Vec<u8> {
    let mut data = vec![0xffu8; CAPACITY as usize];
    let prefix = b"synthetic-not-a-backup!!!";
    data[..prefix.len()].copy_from_slice(prefix);

    let magic: Vec<u8> = IMAGE_MAGIC.iter().flat_map(|w| w.to_le_bytes()).collect();
    for offset in [0x6000, 0x186000] {
        data[offset..offset + 8].copy_from_slice(&magic);
    }

    data[0x3EA000..0x3EA010].copy_from_slice(&(0..16).collect::<Vec<u8>>());
    data[0x7D6000..0x7D6010].copy_from_slice(&(16..32).collect::<Vec<u8>>());

    data
}

struct Reads {
    dir: tempfile::TempDir,
    data: Vec<u8>,
}

impl Reads {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = synthetic();
        fs::write(dir.path().join("read-1.bin"), &data).unwrap();
        fs::write(dir.path().join("read-2.bin"), &data).unwrap();

        Self { dir, data }
    }

    fn first(&self) -> PathBuf {
        self.dir.path().join("read-1.bin")
    }

    fn second(&self) -> PathBuf {
        self.dir.path().join("read-2.bin")
    }

    fn audit_with(
        &self,
        first: &Path,
        second: &Path,
        capacity: u64,
        stock: Option<&Path>,
    ) -> Result<Value> {
        let root = root();
        audit(&Inputs {
            root: &root,
            first,
            second,
            capacity,
            unit_label: "research-feeder",
            stock,
            capacity_evidence: None,
        })
        .map(|report| serde_json::to_value(report).unwrap())
    }

    fn audit(&self) -> Value {
        self.audit_with(&self.first(), &self.second(), CAPACITY, None)
            .unwrap()
    }
}

#[test]
fn identical_full_reads_preserve_unknowns_and_do_not_claim_recovery() {
    let r = Reads::new();
    let report = r.audit();

    assert_eq!(report["comparison_passed"], true);
    assert_eq!(
        report["checks"]["all_observed_preserve_areas_covered"],
        true
    );

    assert_eq!(report["differences"]["different_bytes"], 0);
    assert!(
        report["limits"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v == false)
    );
    assert!(
        report["known_preserve_areas"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["identical"] == true)
    );

    let banks = report["application_banks"]["first"].as_array().unwrap();
    assert!(
        banks
            .iter()
            .all(|b| b["marker_matches_known_magic"] == true && b["currently_active"] == "unknown")
    );

    assert!(report["stock_reference_sha256"].is_null());
    assert_eq!(fs::read(r.first()).unwrap(), r.data);
}

#[test]
fn factory_bank_and_unmapped_changes_are_all_reported_without_values() {
    let mut r = Reads::new();
    for offset in [7, 0x7000, 0x300000, 0x3EA003, 0x7D6001] {
        r.data[offset] ^= 1;
    }

    fs::write(r.second(), &r.data).unwrap();
    let report = r.audit();

    assert_eq!(report["comparison_passed"], false);
    assert_eq!(report["differences"]["different_bytes"], 5);
    assert_eq!(report["changed_sectors"].as_array().unwrap().len(), 5);

    let mut changed: Vec<&str> = report["known_preserve_areas"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["identical"] == false)
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    changed.sort();
    assert_eq!(
        changed,
        [
            "observed_plan_record_sectors",
            "observed_settings_sectors",
            "pre_application"
        ]
    );

    let unknown = report["changed_sectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["start"] == 0x300000)
        .unwrap();
    assert_eq!(unknown["known_preserve_areas"], json!([]));
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("first_byte")
    );
}

#[test]
fn identical_partial_and_padded_uniform_reads_do_not_pass() {
    let r = Reads::new();
    let short = &r.data[..r.data.len() - 4096];
    fs::write(r.first(), short).unwrap();
    fs::write(r.second(), short).unwrap();

    let report = r.audit();
    assert_eq!(report["comparison_passed"], false);
    assert_eq!(report["checks"]["lengths_match_supplied_capacity"], false);

    for value in [0u8, 0xff] {
        fs::write(r.first(), vec![value; CAPACITY as usize]).unwrap();
        fs::write(r.second(), vec![value; CAPACITY as usize]).unwrap();
        assert_eq!(r.audit()["comparison_passed"], false);
    }
}

#[test]
fn alias_pattern_and_uncovered_areas_are_explicit() {
    let r = Reads::new();
    let half = r.data[..r.data.len() / 2].to_vec();
    fs::write(r.first(), [&half[..], &half].concat()).unwrap();
    fs::write(r.second(), [&half[..], &half].concat()).unwrap();

    let report = r.audit();
    assert_eq!(report["comparison_passed"], false);
    assert!(
        report["pattern_anomalies"]["first"]
            .as_array()
            .unwrap()
            .contains(&json!(
                "identical_halves_require_alias_or_content_investigation"
            ))
    );

    let report = r
        .audit_with(&r.first(), &r.second(), CAPACITY / 2, None)
        .unwrap();
    assert_eq!(report["checks"]["lengths_match_supplied_capacity"], false);

    fs::write(r.first(), &half).unwrap();
    fs::write(r.second(), &half).unwrap();
    let report = r
        .audit_with(&r.first(), &r.second(), half.len() as u64, None)
        .unwrap();
    assert_eq!(report["comparison_passed"], false);
    assert_eq!(
        report["checks"]["all_observed_preserve_areas_covered"],
        false
    );
    assert!(
        report["known_preserve_areas"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["covered_in_both"] == false)
    );
}

#[test]
fn same_file_hardlink_invalid_capacity_and_wrong_reference_are_refused() {
    let r = Reads::new();
    let error = |result: Result<Value>| result.unwrap_err().to_string();

    assert!(error(r.audit_with(&r.first(), &r.first(), CAPACITY, None)).contains("same file"));

    fs::remove_file(r.second()).unwrap();
    fs::hard_link(r.first(), r.second()).unwrap();
    assert!(error(r.audit_with(&r.first(), &r.second(), CAPACITY, None)).contains("same file"));

    fs::remove_file(r.second()).unwrap();
    fs::write(r.second(), &r.data).unwrap();

    for capacity in [0, CAPACITY - 1, MAX_BYTES + 4096] {
        assert!(error(r.audit_with(&r.first(), &r.second(), capacity, None)).contains("Capacity"));
    }

    assert!(
        error(r.audit_with(&r.first(), &r.second(), CAPACITY, Some(&r.first())))
            .contains("Reference is not")
    );
}

#[test]
fn report_is_private_exclusive_and_cannot_replace_inputs_or_follow_symlinks() {
    let r = Reads::new();
    let root = root();
    fs::create_dir_all(root.join("private")).unwrap();
    let directory = tempfile::tempdir_in(root.join("private")).unwrap();
    let report_path = directory.path().join("audit.json");

    let report = r.audit();
    write_report(&root, &report_path, &report, &[&r.first(), &r.second()]).unwrap();

    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&report_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    assert!(
        write_report(&root, &report_path, &report, &[])
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );

    let link = directory.path().join("symlink.json");
    let target = directory.path().join("absent.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(
        write_report(&root, &link, &report, &[])
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert!(!target.exists());

    let public = r.dir.path().join("public.json");
    assert!(
        write_report(&root, &public, &json!({}), &[])
            .unwrap_err()
            .to_string()
            .contains("under ignored")
    );
}

#[test]
fn diff_ranges_are_bounded_and_include_missing_tail_bytes() {
    let first = vec![0u8; 1024];
    let second: Vec<u8> = (0..512).flat_map(|_| [1u8, 0]).collect();
    let report = serde_json::to_value(differences(&first, &second)).unwrap();

    assert_eq!(report["different_bytes"], 512);
    assert_eq!(report["total_runs"], 512);
    assert_eq!(report["runs"].as_array().unwrap().len(), MAX_RUNS);
    assert_eq!(report["runs_truncated"], true);

    assert_eq!(
        serde_json::to_value(differences(b"abc", b"abcdef")).unwrap()["runs"],
        json!([{"start": 3, "end_exclusive": 6}])
    );

    let diff = serde_json::to_value(differences(b"abcdef", b"aXcdYYZ")).unwrap();
    assert_eq!(diff["different_bytes"], 4);
    assert_eq!(
        diff["runs"],
        json!([{"start": 1, "end_exclusive": 2}, {"start": 4, "end_exclusive": 7}])
    );
}

#[test]
fn suspicious_reads_are_flagged() {
    assert_eq!(
        suspicious_patterns(&[0xff; 8]),
        [
            "entire_read_is_erased_0xff",
            "identical_halves_require_alias_or_content_investigation"
        ]
    );
    assert!(suspicious_patterns(&[1, 2, 3, 4]).is_empty());
}

/// Needs the private pinned original OTA: `make check-fixtures`.
#[test]
#[ignore = "requires the private pinned original OTA"]
fn bank_body_is_identified_even_when_old_marker_is_invalid_but_not_after_other_changes() {
    let root = root();
    let stock = root.join("private/firmware/PLAF103_OTA_010115.bin");
    let original = reference(Some(&stock)).unwrap().unwrap();

    let mut data = synthetic();
    let payload = &original[32..];
    for start in [0x6000, 0x186000] {
        data[start..start + payload.len()].copy_from_slice(payload);
    }
    data[0x186000..0x186004].copy_from_slice(&[0; 4]);

    let layout: wellfed_formats::layout::Layout =
        wellfed_formats::load(&root.join(LAYOUT)).unwrap();
    let result = serde_json::to_value(banks(&data, &layout, Some(&original)).unwrap()).unwrap();

    assert_eq!(result[0]["marker_matches_known_magic"], true);
    assert_eq!(result[1]["marker_matches_known_magic"], false);
    assert!(
        result
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["image_after_marker_normalization"] == "original-1.1.15")
    );

    data[0x186100] ^= 1;
    assert_eq!(
        serde_json::to_value(banks(&data, &layout, Some(&original)).unwrap()).unwrap()[1]["image_after_marker_normalization"],
        "unrecognized"
    );
}
