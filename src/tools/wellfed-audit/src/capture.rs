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
use wellfed_formats::capture::{Direction, SerialEvent};
use wellfed_formats::session::{Lifecycle, Status};
use serde::Deserialize;
use serde_json::Value;

use wellfed_formats::require;

use crate::{Result, digest};

pub(crate) type Time = DateTime<FixedOffset>;

/// A capture refused before anything is read from it: its files disagree
/// with the finalized report, or its event log does not cover both streams
/// contiguously and in time order.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Timestamp requires a timezone")]
    NoTimezone { text: String },
    /// Not ISO 8601 at all; the message is Python's `fromisoformat` refusal.
    #[error("Invalid isoformat string: {text:?}")]
    Timestamp { text: String },
    /// A record offset no receive chunk of the event log covers.
    #[error("No receive chunk holds offset {offset}")]
    Uncovered { offset: usize },
    /// The parse error stays in the message so the operator sees the line
    /// and the reason together.
    #[error("Invalid serial event on line {line}: {error}")]
    Event {
        line: usize,
        error: serde_json::Error,
    },
    #[error("Capture must be complete")]
    Incomplete,
    #[error("Finalized capture hash missing")]
    HashMissing { direction: Direction },
    #[error("Capture hash mismatch")]
    HashMismatch { direction: Direction },
    #[error("Invalid event extent")]
    Extent { offset: u64 },
    #[error("Noncontiguous event offsets")]
    Noncontiguous { expected: u64, offset: u64 },
    #[error("Event timestamps moved backward")]
    Backward { utc: String },
    #[error("Event coverage mismatch")]
    Coverage,
}

/// An ISO 8601 timestamp that carries its offset, as Python's `fromisoformat`
/// with a required timezone.
pub(crate) fn timestamp(text: &str) -> Result<Time> {
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Ok(time);
    }

    require(
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").is_err(),
        || CaptureError::NoTimezone { text: text.into() },
    )?;
    Err(CaptureError::Timestamp { text: text.into() }.into())
}

/// `(later - earlier).total_seconds()`, from whole microseconds as Python has them.
pub(crate) fn seconds(later: Time, earlier: Time) -> f64 {
    let micros =
        |t: Time| i128::from(t.timestamp()) * 1_000_000 + i128::from(t.timestamp_subsec_micros());
    (micros(later) - micros(earlier)) as f64 / 1e6
}

/// A capture as loaded from disk; it is evidence only after `validate` passes.
pub(crate) struct Capture {
    pub(crate) rx: Vec<u8>,
    pub(crate) tx: Vec<u8>,
    /// Typed on load, so an extent that is not a non-negative integer (a
    /// float, a negative, a bool) or a direction other than rx/tx refuses the
    /// capture before anything is read from it.
    pub(crate) events: Vec<SerialEvent>,
    /// The session's finalized `report.json`. Its shape belongs to the bench
    /// session that writes it; its end time and stream hashes, under the
    /// keys old and new reports use, are read here.
    pub(crate) metadata: Value,
    /// Its status and mode. A status the flash client does not write
    /// reads as none, which the checks that need one refuse.
    pub(crate) lifecycle: Lifecycle,
}

impl Capture {
    pub(crate) fn load(directory: &Path) -> Result<Self> {
        let rx = fs::read(directory.join("serial-rx.bin"))?;
        let tx = fs::read(directory.join("serial-tx.bin"))?;
        let events = fs::read_to_string(directory.join("serial-events.jsonl"))?
            .lines()
            .enumerate()
            .map(|(index, line)| {
                serde_json::from_str(line).map_err(|error| {
                    CaptureError::Event {
                        line: index + 1,
                        error,
                    }
                    .into()
                })
            })
            .collect::<Result<_>>()?;
        let metadata: Value = serde_json::from_slice(&fs::read(directory.join("report.json"))?)?;
        let lifecycle = Lifecycle::deserialize(&metadata).unwrap_or_default();

        Ok(Self {
            rx,
            tx,
            events,
            metadata,
            lifecycle,
        })
    }

    /// The finalized end of the capture.
    pub(crate) fn end(&self) -> Result<Time> {
        timestamp(self.metadata["end_utc"].as_str().unwrap_or_default())
    }

    /// Host receive time of the rx chunk holding `offset`.
    pub(crate) fn arrival(&self, offset: usize) -> Result<Time> {
        let event = u64::try_from(offset)
            .ok()
            .and_then(|at| self.events.iter().find(|e| e.received(at)))
            .ok_or(CaptureError::Uncovered { offset })?;
        timestamp(&event.utc)
    }

    /// Finalized hashes match and the events cover both streams contiguously,
    /// in time order.
    pub(crate) fn validate(&self) -> Result<()> {
        let metadata = &self.metadata;
        require(self.lifecycle.status == Some(Status::Complete), || {
            CaptureError::Incomplete
        })?;

        for (direction, name, data) in [
            (Direction::Rx, "rx", &self.rx),
            (Direction::Tx, "tx", &self.tx),
        ] {
            let hashes: Vec<&Value> = [
                format!("{name}_sha256"),
                format!("serial-{name}.bin_sha256"),
            ]
            .iter()
            .filter_map(|key| metadata.get(key))
            .collect();
            require(!hashes.is_empty(), || CaptureError::HashMissing {
                direction,
            })?;

            let actual = digest(data);
            require(
                hashes
                    .iter()
                    .all(|value| value.as_str() == Some(actual.as_str())),
                || CaptureError::HashMismatch { direction },
            )?;
        }

        let mut offsets = [0u64, 0u64];
        let mut previous: Option<Time> = None;
        for event in &self.events {
            let index = match event.direction {
                Direction::Rx => 0,
                Direction::Tx => 1,
            };
            require(event.bytes > 0, || CaptureError::Extent {
                offset: event.offset,
            })?;
            require(event.offset == offsets[index], || {
                CaptureError::Noncontiguous {
                    expected: offsets[index],
                    offset: event.offset,
                }
            })?;

            let when = timestamp(&event.utc)?;
            require(previous.is_none_or(|p| when >= p), || {
                CaptureError::Backward {
                    utc: event.utc.clone(),
                }
            })?;
            previous = Some(when);
            // Saturating: an absurd extent then fails the coverage check.
            offsets[index] = offsets[index].saturating_add(event.bytes);
        }

        require(
            offsets == [self.rx.len() as u64, self.tx.len() as u64],
            || CaptureError::Coverage.into(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A finalized capture directory of `rx` in chunks arriving `step` seconds apart.
    pub(crate) fn write_capture(dir: &Path, rx: &[u8], chunk: usize, step_micros: i64) {
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
        let refusal = |capture: Capture| match capture.validate() {
            Err(crate::Error::Capture(error)) => error,
            other => panic!("expected a capture refusal, got {other:?}"),
        };

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.rx.push(b'!');
        assert!(matches!(
            refusal(capture),
            CaptureError::HashMismatch {
                direction: Direction::Rx
            }
        ));

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[1].offset = 5;
        assert!(matches!(
            refusal(capture),
            CaptureError::Noncontiguous {
                expected: 4,
                offset: 5
            }
        ));

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[2].utc = "2026-10-22T09:00:00+00:00".into();
        assert!(matches!(refusal(capture), CaptureError::Backward { .. }));

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[0].utc = "2026-10-22T09:00:00".into();
        assert!(matches!(refusal(capture), CaptureError::NoTimezone { .. }));

        let mut capture = Capture::load(tmp.path()).unwrap();
        capture.events[0].bytes = 0;
        assert!(matches!(
            refusal(capture),
            CaptureError::Extent { offset: 0 }
        ));
    }

    /// Operators read this line in refused audits: it names the line and the
    /// parse error together.
    #[test]
    fn an_invalid_event_names_its_line() {
        let tmp = tempfile::tempdir().unwrap();
        write_capture(tmp.path(), b"hello world", 11, 1000);
        fs::write(tmp.path().join("serial-events.jsonl"), "{}\n").unwrap();

        let error = Capture::load(tmp.path()).err().unwrap().to_string();
        assert_eq!(
            error,
            "Invalid serial event on line 1: missing field `utc` at line 1 column 2"
        );
    }

    #[test]
    fn events_that_are_not_integer_extents_or_known_directions_are_refused() {
        let utc = "2026-10-22T10:00:00+00:00";
        for event in [
            json!({"utc": utc, "direction": "rx", "offset": 0, "bytes": true}),
            json!({"utc": utc, "direction": "rx", "offset": 0, "bytes": 11.0}),
            json!({"utc": utc, "direction": "rx", "offset": -1, "bytes": 11}),
            json!({"utc": utc, "direction": "both", "offset": 0, "bytes": 11}),
            json!({"utc": utc, "direction": "rx", "offset": 0}),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            write_capture(tmp.path(), b"hello world", 11, 1000);
            fs::write(tmp.path().join("serial-events.jsonl"), format!("{event}\n")).unwrap();

            let error = Capture::load(tmp.path()).err().unwrap();
            assert!(
                matches!(
                    error,
                    crate::Error::Capture(CaptureError::Event { line: 1, .. })
                ),
                "{event}"
            );
        }
    }
}
