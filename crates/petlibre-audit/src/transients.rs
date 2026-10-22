//! FW30 logic-analyzer captures of the motor driver, checked for any drive.
//!
//! Port of `audit-driver-transients.py`. It reads a `petlibre-bench transients`
//! session: raw sigrok bytes (one byte per sample, bit n = channel Dn) plus a
//! manifest naming the INA, INB, OUTA and OUTB channels. The TC118AH drives
//! the motor only when its inputs differ (forward H/L, reverse L/H); L/L is
//! standby and H/H is brake. Its outputs differ only while driving.
//!
//! Per cycle, a pass needs:
//! - the capture to start with all four lines LOW (feeder unpowered), so the
//!   power-on itself was recorded;
//! - no sample with INA != INB, and none with OUTA != OUTB;
//! - boot mode: the inputs reach H/H (stopped startup's brake) and stay there
//!   to the end; loader mode: the inputs never leave L/L.
//!
//! Every input or output disagreement is reported with its start and length,
//! however short; one sample is a sample period long.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::{Report, Result, require, round};

const LINES: [&str; 4] = ["ina", "inb", "outa", "outb"];
const LIMIT: &str = "Digital levels at the analyzer threshold (about 1.4 V for fx2lafw clones), sampled at the recorded \
    rate; a pulse shorter than one sample period can be missed. Floating driver outputs read LOW.";

/// (value, start, length) for each run of `byte & mask` in the capture.
pub fn runs(data: &[u8], mask: u8) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut current: Option<u8> = None;
    for (index, byte) in data.iter().enumerate() {
        let value = byte & mask;
        if Some(value) != current {
            if let Some(previous) = current {
                out.push((previous, start, index - start));
            }
            current = Some(value);
            start = index;
        }
    }

    if let Some(previous) = current {
        out.push((previous, start, data.len() - start));
    }
    out
}

pub fn check_cycle(data: &[u8], channels: &[u8; 4], mode: &str, samplerate: f64) -> Report {
    let bit: [u8; 4] = channels.map(|c| 1u8 << c);
    let mask = bit.iter().fold(0u8, |m, b| m | b);
    let us = 1e6 / samplerate;
    let mut result = Report::new();
    result.insert("samples".into(), json!(data.len()));
    result.insert(
        "seconds".into(),
        json!(round(data.len() as f64 / samplerate, 6)),
    );

    let mut input_drive = Vec::new();
    let mut output_drive = Vec::new();

    if data.is_empty() {
        result.insert("input_drive".into(), json!([]));
        result.insert("output_drive".into(), json!([]));
        result.insert("outcome".into(), json!("transients_empty"));
        return result;
    }

    let timeline = runs(data, mask);
    let on = |value: u8, line: usize| value & bit[line] != 0;
    for (value, start, length) in &timeline {
        let event = |extra: (&str, &str)| json!({"start_us": round(*start as f64 * us, 3), "length_us": round(*length as f64 * us, 3), extra.0: extra.1});
        if on(*value, 0) != on(*value, 1) {
            input_drive.push(event((
                "direction",
                if on(*value, 0) { "forward" } else { "reverse" },
            )));
        }
        if on(*value, 2) != on(*value, 3) {
            output_drive.push(event(("high", if on(*value, 2) { "outa" } else { "outb" })));
        }
    }

    let inputs: Vec<(bool, bool, usize)> = timeline
        .iter()
        .map(|(v, s, _)| (on(*v, 0), on(*v, 1), *s))
        .collect();
    let brake_at = inputs.iter().find(|(a, b, _)| *a && *b).map(|(_, _, s)| *s);

    result.insert("input_drive".into(), json!(input_drive));
    result.insert("output_drive".into(), json!(output_drive));
    result.insert(
        "brake_at_s".into(),
        brake_at.map_or(Value::Null, |s| json!(round(s as f64 / samplerate, 6))),
    );
    result.insert("transitions".into(), json!(timeline.len() - 1));

    let outcome = if timeline[0].0 != 0 {
        "transients_late_start"
    } else if !input_drive.is_empty() {
        "transients_input_drive"
    } else if !output_drive.is_empty() {
        "transients_output_drive"
    } else if mode == "boot" {
        let (a, b, _) = inputs[inputs.len() - 1];
        match brake_at {
            Some(at) if a && b => {
                // After the first brake the inputs must not return to standby.
                if inputs.iter().any(|(x, y, s)| *s > at && !(*x && *y)) {
                    "transients_left_brake"
                } else {
                    "pass"
                }
            }
            _ => "transients_no_brake",
        }
    } else if brake_at.is_some() {
        "transients_loader_driven"
    } else {
        "pass"
    };

    result.insert("outcome".into(), json!(outcome));
    result
}

pub fn verify(capture: &Path) -> Result<Report> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(capture.join("manifest.json"))?)?;
    let mode = manifest["mode"].as_str().unwrap_or("");
    require(
        manifest["schema_version"] == 1 && ["boot", "loader"].contains(&mode),
        "Unsupported transients manifest",
    )?;

    let channel = |name: &str| {
        manifest["channels"][name]
            .as_u64()
            .filter(|c| *c < 8)
            .map(|c| c as u8)
    };
    let channels: Option<Vec<u8>> = LINES.iter().map(|n| channel(n)).collect();
    let channels = channels
        .and_then(|c| <[u8; 4]>::try_from(c).ok())
        .filter(|c| {
            let mut distinct = c.to_vec();
            distinct.sort();
            distinct.dedup();
            distinct.len() == 4
        })
        .ok_or_else(|| {
            crate::Error::Refused(
                "Manifest needs four distinct channels 0-7 for INA, INB, OUTA and OUTB".into(),
            )
        })?;

    let samplerate = manifest["samplerate"].as_f64().unwrap_or(0.0);
    require(samplerate > 0.0, "Unsupported transients manifest")?;

    let mut cycles = Vec::new();
    for entry in manifest["cycles"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let file = entry["file"].as_str().unwrap_or("");
        require(
            Path::new(file).components().count() == 1 && !file.is_empty(),
            "Unsupported transients manifest",
        )?;
        let data = fs::read(capture.join(file))?;
        require(
            entry["bytes"] == data.len(),
            format!("{file} changed since capture"),
        )?;

        let mut cycle = Report::new();
        cycle.insert("file".into(), json!(file));
        crate::update(&mut cycle, &check_cycle(&data, &channels, mode, samplerate));
        cycles.push(cycle);
    }

    let passed = cycles.iter().filter(|c| c["outcome"] == "pass").count();
    let brakes: Vec<f64> = cycles
        .iter()
        .filter_map(|c| c.get("brake_at_s").and_then(Value::as_f64))
        .collect();
    let events = |key: &str| -> usize {
        cycles
            .iter()
            .map(|c| c[key].as_array().map_or(0, Vec::len))
            .sum()
    };

    let brake = if brakes.is_empty() {
        Value::Null
    } else {
        json!({"min": brakes.iter().copied().fold(f64::INFINITY, f64::min),
               "max": brakes.iter().copied().fold(f64::NEG_INFINITY, f64::max)})
    };

    let report = json!({
        "outcome": if !cycles.is_empty() && passed == cycles.len() { "pass" } else { "transients_fail" },
        "mode": mode, "samplerate": manifest["samplerate"], "cycles": cycles.len(), "passed": passed,
        "input_drive_events": events("input_drive"), "output_drive_events": events("output_drive"),
        "brake_at_s": brake, "per_cycle": cycles, "limit": LIMIT,
    });

    Ok(match report {
        Value::Object(map) => map,
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests;
