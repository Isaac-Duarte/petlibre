//! The FW30 transient auditor on synthetic captures; no analyzer or device.

use serde_json::json;

use super::*;
use TransientMode::{Boot, Loader};

const CH: [u8; 4] = [0, 1, 2, 3];
const OFF: u8 = 0b0000;
const BRAKE: u8 = 0b0011;
const FWD: u8 = 0b0001;
const REV: u8 = 0b0010;

const RATE: f64 = 1_000_000.0;

/// Segments of (value, samples); bits 4-7 carry unrelated noise.
fn capture(segments: &[(u8, usize)]) -> Vec<u8> {
    segments
        .iter()
        .flat_map(|(value, n)| std::iter::repeat_n(value | 0x50, *n))
        .collect()
}

fn check(data: &[u8], mode: TransientMode) -> Report {
    check_cycle(data, &CH, mode, RATE)
}

#[test]
fn clean_boot_and_loader() {
    let r = check(&capture(&[(OFF, 3_000_000), (BRAKE, 2_000_000)]), Boot);
    assert_eq!(
        (&r["outcome"], &r["brake_at_s"]),
        (&json!("pass"), &json!(3.0))
    );

    assert_eq!(
        check(&capture(&[(OFF, 5_000_000)]), Loader)["outcome"],
        "pass"
    );
}

#[test]
fn a_single_sample_drive_is_caught_and_timed() {
    let r = check(
        &capture(&[(OFF, 1000), (FWD, 1), (OFF, 10), (BRAKE, 100)]),
        Boot,
    );
    assert_eq!(r["outcome"], "transients_input_drive");
    assert_eq!(
        r["input_drive"],
        json!([{"start_us": 1000.0, "length_us": 1.0, "direction": "forward"}])
    );

    let r = check(&capture(&[(OFF, 10), (REV, 5), (BRAKE, 10)]), Boot);
    assert_eq!(r["input_drive"][0]["direction"], "reverse");

    // Inputs equal but an output pulse (OUTA high) is still a drive.
    let r = check(&capture(&[(OFF, 10), (0b0100, 2), (BRAKE, 10)]), Boot);
    assert_eq!(
        (&r["outcome"], &r["output_drive"][0]["high"]),
        (&json!("transients_output_drive"), &json!("outa"))
    );
}

#[test]
fn state_rules() {
    for (data, mode, outcome) in [
        (capture(&[(BRAKE, 10)]), Boot, "transients_late_start"),
        (capture(&[(OFF, 100)]), Boot, "transients_no_brake"),
        (
            capture(&[(OFF, 10), (BRAKE, 10), (OFF, 10)]),
            Boot,
            "transients_no_brake",
        ),
        (
            capture(&[(OFF, 10), (BRAKE, 10), (OFF, 3), (BRAKE, 10)]),
            Boot,
            "transients_left_brake",
        ),
        (
            capture(&[(OFF, 10), (BRAKE, 10)]),
            Loader,
            "transients_loader_driven",
        ),
        (vec![], Boot, "transients_empty"),
    ] {
        assert_eq!(check(&data, mode)["outcome"], outcome);
    }
}

#[test]
fn session_summary_and_integrity() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let files = [
        ("cycle-001.bin", capture(&[(OFF, 2000), (BRAKE, 1000)])),
        ("cycle-002.bin", capture(&[(OFF, 2500), (BRAKE, 500)])),
    ];
    for (name, data) in &files {
        fs::write(d.join(name), data).unwrap();
    }

    let mut manifest = json!({"schema_version": 1, "mode": "boot", "samplerate": 1_000_000, "seconds": 8,
        "arming_seconds": 3.0, "channels": {"ina": 0, "inb": 1, "outa": 2, "outb": 3},
        "cycles": files.iter().map(|(n, b)| json!({"file": n, "bytes": b.len(),
                                                   "power_on_after_launch_s": 3.2}))
                       .collect::<Vec<_>>()});
    fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();

    let report = verify(d).unwrap();
    assert_eq!(
        (&report["outcome"], &report["passed"]),
        (&json!("pass"), &json!(2))
    );
    assert_eq!(report["brake_at_s"], json!({"min": 0.002, "max": 0.0025}));

    fs::write(d.join("cycle-002.bin"), [&files[1].1[..], b"\0"].concat()).unwrap();
    assert!(
        verify(d)
            .unwrap_err()
            .to_string()
            .contains("changed since capture")
    );

    manifest["channels"] = json!({"ina": 0, "inb": 0, "outa": 2, "outb": 3});
    fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
    assert!(
        verify(d)
            .unwrap_err()
            .to_string()
            .contains("four distinct channels")
    );

    manifest["cycles"][0]["file"] = json!("../outside.bin");
    manifest["channels"] = json!({"ina": 0, "inb": 1, "outa": 2, "outb": 3});
    fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
    assert!(verify(d).is_err());
}
