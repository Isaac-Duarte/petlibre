//! A finalized receive-only capture: `serial-rx.bin`, `serial-tx.bin`,
//! `serial-events.jsonl` and `report.json`, validated as
//! `analyze` in `audit-uart-startups.py` does before anything is read from it.
//!
//! Nothing is read until the hashes match the finalized report and the event
//! log covers both streams contiguously and in time order; any gap, overlap or
//! mismatch refuses the capture instead of auditing part of it.

use std::fs;
use std::path::Path;

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use crate::{Result, digest, require};

pub type Time = DateTime<FixedOffset>;

/// An ISO 8601 timestamp that carries its offset, as Python's `fromisoformat`
/// with a required timezone.
pub fn timestamp(value: &Value) -> Result<Time> {
    let text = value.as_str().unwrap_or("");
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Ok(time);
    }

    require(
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").is_err(),
        "Timestamp requires a timezone",
    )?;
    Err(crate::Error::Refused(format!(
        "Invalid isoformat string: {text:?}"
    )))
}

/// `(later - earlier).total_seconds()`, from whole microseconds as Python has them.
pub fn seconds(later: Time, earlier: Time) -> f64 {
    let micros =
        |t: Time| t.timestamp() as i128 * 1_000_000 + i128::from(t.timestamp_subsec_micros());
    (micros(later) - micros(earlier)) as f64 / 1e6
}

/// A capture as loaded from disk; it is evidence only after `validate` passes.
pub struct Capture {
    pub rx: Vec<u8>,
    pub tx: Vec<u8>,
    pub events: Vec<Value>,
    pub metadata: Value,
    /// Receive chunks: (start, end, utc).
    chunks: Vec<(usize, usize, Value)>,
}

impl Capture {
    pub fn load(directory: &Path) -> Result<Self> {
        let rx = fs::read(directory.join("serial-rx.bin"))?;
        let tx = fs::read(directory.join("serial-tx.bin"))?;
        let events = fs::read_to_string(directory.join("serial-events.jsonl"))?
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<Vec<Value>, _>>()?;
        let metadata: Value = serde_json::from_slice(&fs::read(directory.join("report.json"))?)?;

        let chunks = events
            .iter()
            .filter(|e| e["direction"] == "rx")
            .filter_map(|e| {
                let start = e["offset"].as_u64()? as usize;
                Some((
                    start,
                    start + e["bytes"].as_u64()? as usize,
                    e["utc"].clone(),
                ))
            })
            .collect();

        Ok(Self {
            rx,
            tx,
            events,
            metadata,
            chunks,
        })
    }

    /// Host receive time of the rx chunk holding `offset`.
    pub fn arrival(&self, offset: usize) -> Result<Time> {
        let utc = self
            .chunks
            .iter()
            .find(|(start, end, _)| *start <= offset && offset < *end)
            .map(|(_, _, utc)| utc)
            .ok_or_else(|| {
                crate::Error::Refused(format!("No receive chunk holds offset {offset}"))
            })?;
        timestamp(utc)
    }

    /// Finalized hashes match and the events cover both streams contiguously,
    /// in time order.
    pub fn validate(&self) -> Result<()> {
        let metadata = &self.metadata;
        require(metadata["status"] == "complete", "Capture must be complete")?;

        for (direction, data) in [("rx", &self.rx), ("tx", &self.tx)] {
            let hashes: Vec<&Value> = [
                format!("{direction}_sha256"),
                format!("serial-{direction}.bin_sha256"),
            ]
            .iter()
            .filter_map(|key| metadata.get(key))
            .collect();
            require(!hashes.is_empty(), "Finalized capture hash missing")?;
            let actual = digest(data);
            require(
                hashes
                    .iter()
                    .all(|value| value.as_str() == Some(actual.as_str())),
                "Capture hash mismatch",
            )?;
        }

        let mut offsets = [0i64, 0i64];
        let mut previous: Option<Time> = None;
        for event in &self.events {
            let index = match event.get("direction").and_then(Value::as_str) {
                Some("rx") => 0,
                Some("tx") => 1,
                _ => return Err(crate::Error::Refused("Invalid event direction".into())),
            };

            let (size, offset) = (event.get("bytes"), event.get("offset"));
            let int = |v: Option<&Value>| {
                v.and_then(|v| {
                    if v.is_i64() || v.is_u64() {
                        v.as_i64()
                    } else {
                        None
                    }
                })
            };
            let (Some(size), Some(offset)) = (int(size), int(offset)) else {
                return Err(crate::Error::Refused("Invalid event extent".into()));
            };
            require(size > 0, "Invalid event extent")?;
            require(offset == offsets[index], "Noncontiguous event offsets")?;

            let when = timestamp(&event["utc"])?;
            require(
                previous.is_none_or(|p| when >= p),
                "Event timestamps moved backward",
            )?;
            previous = Some(when);
            offsets[index] += size;
        }

        require(
            offsets == [self.rx.len() as i64, self.tx.len() as i64],
            "Event coverage mismatch",
        )
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;

    /// A finalized capture directory of `rx` in chunks arriving `step` seconds apart.
    pub fn write_capture(dir: &Path, rx: &[u8], chunk: usize, step_micros: i64) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("serial-rx.bin"), rx).unwrap();
        fs::write(dir.join("serial-tx.bin"), b"").unwrap();

        let start = DateTime::parse_from_rfc3339("2026-10-22T10:00:00+00:00").unwrap();
        let mut events = String::new();
        let mut at = start;
        for (index, piece) in rx.chunks(chunk.max(1)).enumerate() {
            at = start + chrono::Duration::microseconds(step_micros * index as i64);
            events += &format!(
                "{}\n",
                json!({"utc": at.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string(), "direction": "rx",
                       "offset": index * chunk.max(1), "bytes": piece.len()})
            );
        }

        fs::write(dir.join("serial-events.jsonl"), events).unwrap();
        let end = at + chrono::Duration::seconds(1);
        let report = json!({"status": "complete", "mode": "observe", "rx_sha256": digest(rx),
                            "tx_sha256": digest(b""), "end_utc": end.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()});
        fs::write(dir.join("report.json"), report.to_string()).unwrap();
    }

    #[test]
    fn a_complete_capture_validates() {
        let tmp = tempfile::tempdir().unwrap();
        write_capture(tmp.path(), b"hello world", 4, 1000);

        let capture = Capture::load(tmp.path()).unwrap();
        capture.validate().unwrap();
        assert_eq!(
            seconds(capture.arrival(8).unwrap(), capture.arrival(0).unwrap()),
            0.002
        );
    }

    #[test]
    fn hash_gaps_and_time_order_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        write_capture(tmp.path(), b"hello world", 4, 1000);

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.rx.push(b'!');
        assert!(
            capture
                .validate()
                .unwrap_err()
                .to_string()
                .contains("hash mismatch")
        );

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[1]["offset"] = json!(5);
        assert!(
            capture
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Noncontiguous")
        );

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[2]["utc"] = json!("2026-10-22T09:00:00+00:00");
        assert!(
            capture
                .validate()
                .unwrap_err()
                .to_string()
                .contains("backward")
        );

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[0]["utc"] = json!("2026-10-22T09:00:00");
        assert!(
            capture
                .validate()
                .unwrap_err()
                .to_string()
                .contains("timezone")
        );

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[0]["bytes"] = json!(true);
        assert!(
            capture
                .validate()
                .unwrap_err()
                .to_string()
                .contains("extent")
        );
    }
}
