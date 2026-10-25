//! `serial-events.jsonl`: one record per chunk a UART evidence session
//! received or sent, locating it in `serial-rx.bin`/`serial-tx.bin`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Rx,
    Tx,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialEvent {
    /// RFC 3339 host time the chunk was read or written.
    pub utc: String,
    pub direction: Direction,
    /// Where the chunk starts in its direction's stream file.
    pub offset: u64,
    pub bytes: u64,
}

impl SerialEvent {
    /// Whether this is the received chunk that holds stream byte `offset`.
    pub fn received(&self, offset: u64) -> bool {
        self.direction == Direction::Rx
            && self.offset <= offset
            && offset < self.offset.saturating_add(self.bytes)
    }

    /// Every well-formed record of a `serial-events.jsonl`, skipping lines
    /// that do not parse: readers here only locate times, while the capture
    /// audit validates the file line by line.
    pub fn parse_lines(text: &str) -> impl Iterator<Item = Self> + '_ {
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
    }
}

/// When the receive byte at `offset` arrived, in Unix seconds, from a
/// capture's `serial-events.jsonl` (one timed record per received chunk).
pub fn arrival(events: &str, offset: u64) -> Option<f64> {
    let event = SerialEvent::parse_lines(events).find(|event| event.received(offset))?;
    let utc = chrono::DateTime::parse_from_rfc3339(&event.utc).ok()?;
    Some(utc.timestamp_micros() as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_received_bytes() {
        let event = SerialEvent {
            utc: "2026-10-22T00:00:00Z".into(),
            direction: Direction::Rx,
            offset: 4,
            bytes: 2,
        };
        assert!(!event.received(3));
        assert!(event.received(5));
        assert!(!event.received(6));
    }

    #[test]
    fn arrival_reads_the_chunk_holding_an_offset() {
        let chunks = concat!(
            r#"{"utc":"2026-10-22T12:00:00+00:00","direction":"rx","offset":0,"bytes":10}"#,
            "\n",
            r#"{"utc":"2026-10-22T12:00:01.5+00:00","direction":"tx","offset":0,"bytes":99}"#,
            "\n",
            r#"{"utc":"2026-10-22T12:00:02.250000+00:00","direction":"rx","offset":10,"bytes":5}"#,
        );
        let noon = arrival(chunks, 0).unwrap();
        assert_eq!(arrival(chunks, 9), Some(noon));
        assert_eq!(arrival(chunks, 14), Some(noon + 2.25));
        assert_eq!(arrival(chunks, 15), None);
        assert_eq!(arrival("", 0), None);
    }
}
