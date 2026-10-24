//! Logic-analyzer captures of the motor driver, checked for any drive.
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

use petlibre_formats::transients::{TransientMode, TransientsManifest};
use serde::Serialize;

use crate::{Error, Result, outcomes, require, round};

const LINES: [&str; 4] = ["ina", "inb", "outa", "outb"];
const LIMIT: &str = "Digital levels at the analyzer threshold (about 1.4 V for fx2lafw clones), sampled at the recorded \
    rate; a pulse shorter than one sample period can be missed. Floating driver outputs read LOW.";

outcomes! {
    /// A cycle's outcome, or the session's (`pass` only when every cycle passed).
    pub enum TransientOutcome {
        Pass => "pass",
        Empty => "transients_empty",
        LateStart => "transients_late_start",
        InputDrive => "transients_input_drive",
        OutputDrive => "transients_output_drive",
        NoBrake => "transients_no_brake",
        LeftBrake => "transients_left_brake",
        LoaderDriven => "transients_loader_driven",
        Fail => "transients_fail",
    }
}

/// (value, start, length) for each run of `byte & mask` in the capture.
pub(crate) fn runs(data: &[u8], mask: u8) -> Vec<(u8, usize, usize)> {
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

/// Which way differing inputs drive the motor.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Direction {
    Forward,
    Reverse,
}

/// The driver output that was high while the other was low.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Output {
    Outa,
    Outb,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Drive {
    Direction(Direction),
    High(Output),
}

/// One run of differing inputs or outputs.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DriveEvent {
    start_us: f64,
    length_us: f64,
    #[serde(flatten)]
    drive: Drive,
}

/// What a non-empty capture's timeline showed.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Timeline {
    /// When the inputs first reached H/H, if they did.
    brake_at_s: Option<f64>,
    transitions: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CycleCheck {
    samples: usize,
    seconds: f64,
    input_drive: Vec<DriveEvent>,
    output_drive: Vec<DriveEvent>,
    #[serde(flatten)]
    timeline: Option<Timeline>,
    outcome: TransientOutcome,
}

pub(crate) fn check_cycle(
    data: &[u8],
    channels: &[u8; 4],
    mode: TransientMode,
    samplerate: f64,
) -> CycleCheck {
    let bit: [u8; 4] = channels.map(|c| 1u8 << c);
    let mask = bit.iter().fold(0u8, |m, b| m | b);
    let us = 1e6 / samplerate;
    let mut check = CycleCheck {
        samples: data.len(),
        seconds: round(data.len() as f64 / samplerate, 6),
        input_drive: Vec::new(),
        output_drive: Vec::new(),
        timeline: None,
        outcome: TransientOutcome::Empty,
    };
    if data.is_empty() {
        return check;
    }

    let timeline = runs(data, mask);
    let on = |value: u8, line: usize| value & bit[line] != 0;
    for &(value, start, length) in &timeline {
        let event = |drive| DriveEvent {
            start_us: round(start as f64 * us, 3),
            length_us: round(length as f64 * us, 3),
            drive,
        };
        if on(value, 0) != on(value, 1) {
            check
                .input_drive
                .push(event(Drive::Direction(if on(value, 0) {
                    Direction::Forward
                } else {
                    Direction::Reverse
                })));
        }
        if on(value, 2) != on(value, 3) {
            check.output_drive.push(event(Drive::High(if on(value, 2) {
                Output::Outa
            } else {
                Output::Outb
            })));
        }
    }

    let inputs: Vec<(bool, bool, usize)> = timeline
        .iter()
        .map(|(v, s, _)| (on(*v, 0), on(*v, 1), *s))
        .collect();
    let brake_at = inputs.iter().find(|(a, b, _)| *a && *b).map(|(_, _, s)| *s);
    check.timeline = Some(Timeline {
        brake_at_s: brake_at.map(|s| round(s as f64 / samplerate, 6)),
        transitions: timeline.len() - 1,
    });

    check.outcome = if timeline[0].0 != 0 {
        TransientOutcome::LateStart
    } else if !check.input_drive.is_empty() {
        TransientOutcome::InputDrive
    } else if !check.output_drive.is_empty() {
        TransientOutcome::OutputDrive
    } else if mode == TransientMode::Boot {
        let (a, b, _) = inputs[inputs.len() - 1];
        match brake_at {
            // After the first brake the inputs must not return to standby.
            Some(at) if a && b => {
                if inputs.iter().any(|(x, y, s)| *s > at && !(*x && *y)) {
                    TransientOutcome::LeftBrake
                } else {
                    TransientOutcome::Pass
                }
            }
            _ => TransientOutcome::NoBrake,
        }
    } else if brake_at.is_some() {
        TransientOutcome::LoaderDriven
    } else {
        TransientOutcome::Pass
    };
    check
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Cycle {
    file: String,
    #[serde(flatten)]
    check: CycleCheck,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Range {
    min: f64,
    max: f64,
}

/// The session's totals, printed without the per-cycle detail.
#[derive(Debug, Clone, Serialize)]
pub struct TransientsSummary {
    outcome: TransientOutcome,
    mode: TransientMode,
    samplerate: u64,
    cycles: usize,
    passed: usize,
    input_drive_events: usize,
    output_drive_events: usize,
    brake_at_s: Option<Range>,
    limit: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransientsReport {
    #[serde(flatten)]
    pub summary: TransientsSummary,
    per_cycle: Vec<Cycle>,
}

impl TransientsReport {
    pub fn passed(&self) -> bool {
        self.summary.outcome == TransientOutcome::Pass
    }
}

/// The four driver lines' analyzer channels: distinct, each 0-7.
fn channels(manifest: &TransientsManifest) -> Result<[u8; 4]> {
    let channel = |name: &str| {
        manifest
            .channels
            .get(name)
            .and_then(|&c| u8::try_from(c).ok())
            .filter(|c| *c < 8)
    };
    let channels: Option<Vec<u8>> = LINES.iter().map(|n| channel(n)).collect();
    channels
        .and_then(|c| <[u8; 4]>::try_from(c).ok())
        .filter(|c| {
            let mut distinct = c.to_vec();
            distinct.sort();
            distinct.dedup();
            distinct.len() == 4
        })
        .ok_or_else(|| {
            Error::Refused(
                "Manifest needs four distinct channels 0-7 for INA, INB, OUTA and OUTB".into(),
            )
        })
}

pub fn verify(capture: &Path) -> Result<TransientsReport> {
    let manifest: TransientsManifest = petlibre_formats::load(&capture.join("manifest.json"))?;
    require(
        manifest.schema_version == 1,
        "Unsupported transients manifest",
    )?;
    let channels = channels(&manifest)?;
    require(manifest.samplerate > 0, "Unsupported transients manifest")?;
    let samplerate = manifest.samplerate as f64;

    let mut cycles = Vec::new();
    for entry in &manifest.cycles {
        let file = entry.file.as_str();
        require(
            Path::new(file).components().count() == 1 && !file.is_empty(),
            "Unsupported transients manifest",
        )?;
        let data = fs::read(capture.join(file))?;
        require(
            entry.bytes == data.len() as u64,
            format!("{file} changed since capture"),
        )?;

        cycles.push(Cycle {
            file: file.into(),
            check: check_cycle(&data, &channels, manifest.mode, samplerate),
        });
    }

    let passed = cycles
        .iter()
        .filter(|c| c.check.outcome == TransientOutcome::Pass)
        .count();
    let brakes: Vec<f64> = cycles
        .iter()
        .filter_map(|c| c.check.timeline.as_ref()?.brake_at_s)
        .collect();
    let brake_at_s = (!brakes.is_empty()).then(|| Range {
        min: brakes.iter().copied().fold(f64::INFINITY, f64::min),
        max: brakes.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    });

    let summary = TransientsSummary {
        outcome: if !cycles.is_empty() && passed == cycles.len() {
            TransientOutcome::Pass
        } else {
            TransientOutcome::Fail
        },
        mode: manifest.mode,
        samplerate: manifest.samplerate,
        cycles: cycles.len(),
        passed,
        input_drive_events: cycles.iter().map(|c| c.check.input_drive.len()).sum(),
        output_drive_events: cycles.iter().map(|c| c.check.output_drive.len()).sum(),
        brake_at_s,
        limit: LIMIT,
    };
    Ok(TransientsReport {
        summary,
        per_cycle: cycles,
    })
}

#[cfg(test)]
mod tests;
