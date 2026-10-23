//! Shared test fixtures: synthetic captures and source-bound builds.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Duration};
use wellfed_formats::manifest::BuildManifest;
use serde_json::{Value, json};

use crate::capture::Time;
use crate::startup::{find, verify};
use crate::{Report, Result, digest};

pub fn t0() -> Time {
    DateTime::parse_from_rfc3339("2026-10-21T10:00:00+00:00").unwrap()
}

pub fn at(seconds: f64) -> Time {
    t0() + Duration::microseconds((seconds * 1e6).round() as i64)
}

pub fn replace(data: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    replace_n(data, old, new, usize::MAX)
}

pub fn replace_n(data: &[u8], old: &[u8], new: &[u8], limit: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut rest, mut done) = (data, 0);
    while done < limit
        && let Some(i) = find(rest, old)
    {
        out.extend_from_slice(&rest[..i]);
        out.extend_from_slice(new);
        rest = &rest[i + old.len()..];
        done += 1;
    }
    out.extend_from_slice(rest);
    out
}

pub fn outcome(report: &Report) -> &str {
    report["outcome"].as_str().unwrap()
}

pub const LED_PINS: [&str; 4] = ["PA0", "PA2", "PA4", "PB22"];

pub fn led_full(version: u8) -> Vec<Vec<u8>> {
    let mut records = vec![if version >= 3 {
        b"PLLED1 begin output_only=PA0,PA2,PA4\r\n".to_vec()
    } else {
        b"PLLED1 begin\r\n".to_vec()
    }];

    for pin in LED_PINS {
        let status = if version >= 3 && pin != "PB22" {
            "output"
        } else {
            "pass"
        };
        for level in ["1", "0", "1", "0"] {
            records
                .push(format!("PLLED1 pin={pin} level={level} status={status}\r\n").into_bytes());
        }
    }

    records.push(b"PLLED1 done low=all\r\n".to_vec());
    records
}

/// A capture directory with a source-bound manifest and image, rewritten per case.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub manifest: Value,
    pub identity: String,
}

impl Fixture {
    pub fn new(inputs: Value) -> Self {
        let mut manifest = json!({"profile": "plaf103-firstboot", "inputs": inputs});
        let identity = digest(&wellfed_formats::canonical::canonical(&manifest).unwrap());
        manifest["startup_identity"] = json!(identity);

        Self {
            dir: tempfile::tempdir().unwrap(),
            manifest,
            identity,
        }
    }

    /// The manifest as the audit loads it.
    pub fn typed(&self) -> BuildManifest {
        serde_json::from_value(self.manifest.clone()).unwrap()
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn set(&mut self, key: &str, value: Value) {
        if value.is_null() {
            self.manifest.as_object_mut().unwrap().remove(key);
        } else {
            self.manifest[key] = value;
        }
        fs::write(self.path().join("manifest.json"), self.manifest.to_string()).unwrap();
    }

    pub fn image(&mut self, image: &[u8]) {
        self.manifest["artifacts"] =
            json!([{"path": "OTA_All.bin", "sha256": digest(image), "bytes": image.len()}]);
        fs::write(self.path().join("image.bin"), image).unwrap();
        fs::write(self.path().join("manifest.json"), self.manifest.to_string()).unwrap();
    }

    /// Receive chunks at host seconds after T0, and the capture end.
    pub fn capture(&self, chunks: &[(f64, Vec<u8>)], end: f64) {
        let mut sorted: Vec<&(f64, Vec<u8>)> = chunks.iter().collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let rx: Vec<u8> = sorted.iter().flat_map(|c| c.1.clone()).collect();

        let mut events = String::new();
        let mut offset = 0;
        for (seconds, data) in sorted {
            let utc = at(*seconds)
                .format("%Y-%m-%dT%H:%M:%S%.6f+00:00")
                .to_string();
            events += &format!(
                "{}\n",
                json!({"utc": utc, "direction": "rx", "offset": offset, "bytes": data.len()})
            );
            offset += data.len();
        }

        let p = self.path();
        fs::write(p.join("serial-rx.bin"), &rx).unwrap();
        fs::write(p.join("serial-tx.bin"), b"").unwrap();
        fs::write(p.join("serial-events.jsonl"), events).unwrap();

        let end = at(end).format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string();
        fs::write(
            p.join("report.json"),
            json!({"mode": "observe", "status": "complete", "end_utc": end,
                   "serial-rx.bin_sha256": digest(&rx), "serial-tx.bin_sha256": digest(b"")})
            .to_string(),
        )
        .unwrap();
    }

    pub fn verify(&self, minimum: f64) -> Result<Report> {
        let p = self.path();
        verify(p, &p.join("manifest.json"), &p.join("image.bin"), minimum)
    }

    pub fn error(&self, minimum: f64) -> String {
        self.verify(minimum).unwrap_err().to_string()
    }
}
