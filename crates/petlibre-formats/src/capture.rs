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
}
