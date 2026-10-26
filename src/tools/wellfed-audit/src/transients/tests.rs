//! The transient auditor on synthetic captures; no analyzer or device.

use serde_json::{Value, json};

use super::*;
use TransientMode::{Boot, Loader};

const DRIVER: Probes = Probes {
    inputs: Some([0, 1]),
    outputs: [2, 3],
};
const OUTPUTS: Probes = Probes {
    inputs: None,
    outputs: [2, 3],
};
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

fn check(data: &[u8], mode: TransientMode) -> Value {
    serde_json::to_value(check_cycle(data, &DRIVER, mode, RATE)).unwrap()
}

fn verify(capture: &Path) -> Result<Value> {
    super::verify(capture).map(|report| serde_json::to_value(report).unwrap())
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
    assert!(matches!(
        verify(d),
        Err(crate::Error::Transients(TransientsError::Changed { file })) if file == "cycle-002.bin"
    ));

    manifest["channels"] = json!({"ina": 0, "inb": 0, "outa": 2, "outb": 3});
    fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
    assert!(matches!(
        verify(d),
        Err(crate::Error::Transients(TransientsError::Channels))
    ));

    manifest["cycles"][0]["file"] = json!("../outside.bin");
    manifest["channels"] = json!({"ina": 0, "inb": 1, "outa": 2, "outb": 3});
    fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
    assert!(matches!(
        verify(d),
        Err(crate::Error::Transients(TransientsError::CycleFile { .. }))
    ));
}

#[test]
fn outputs_alone_check_only_the_drive() {
    let outputs =
        |data: &[u8], mode| serde_json::to_value(check_cycle(data, &OUTPUTS, mode, RATE)).unwrap();

    // The input bits are not probed, so their noise is ignored, and no brake
    // time can be read from the outputs.
    let r = outputs(
        &capture(&[(OFF, 100), (BRAKE, 100), (FWD, 3), (OFF, 100)]),
        Boot,
    );
    assert_eq!(
        (&r["outcome"], &r["brake_at_s"]),
        (&json!("pass"), &Value::Null)
    );
    assert_eq!(r["input_drive"], json!([]));

    let r = outputs(&capture(&[(OFF, 10), (0b1000, 4), (OFF, 10)]), Loader);
    assert_eq!(
        (&r["outcome"], &r["output_drive"][0]["high"]),
        (&json!("transients_output_drive"), &json!("outb"))
    );
}

#[test]
fn an_outputs_only_session_says_what_it_did_not_see() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let data = capture(&[(OFF, 3000)]);
    fs::write(d.join("cycle-001.bin"), &data).unwrap();

    let mut manifest = json!({"schema_version": 1, "mode": "boot", "samplerate": 1_000_000, "seconds": 8,
        "arming_seconds": 3.0, "channels": {"outa": 2, "outb": 3},
        "cycles": [{"file": "cycle-001.bin", "bytes": data.len(), "power_on_after_launch_s": 3.2}]});
    fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();

    let report = verify(d).unwrap();
    assert_eq!(
        (&report["outcome"], &report["probed"]),
        (&json!("pass"), &json!("outputs"))
    );
    assert!(
        report["not_probed"]
            .as_str()
            .unwrap()
            .contains("not probed")
    );

    // One input without the other, or an unknown line, is refused.
    for channels in [
        json!({"ina": 0, "outa": 2, "outb": 3}),
        json!({"outa": 2, "outb": 2}),
        json!({"outa": 2, "outb": 3, "power": 4}),
    ] {
        manifest["channels"] = channels;
        fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
        assert!(verify(d).is_err());
    }
}
