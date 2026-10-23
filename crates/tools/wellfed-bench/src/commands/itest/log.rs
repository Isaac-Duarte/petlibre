//! The feeder's UART as a searchable log. Every scenario asserts on the
//! firmware's `PLxxxN` records (`PLMQT1 request op=ping ok=1`), which can sit
//! after SDK noise on the same line; anything else is kept for reading back.
//! Times come from the rig's clock when the bytes were read, so they are as
//! late as the polling interval.

pub(super) use wellfed_core::record::Record;

/// A longer run without a line break is noise, not a record.
const MAX_LINE: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Line {
    pub at: f64,
    pub text: String,
}

#[derive(Debug, Default)]
pub(super) struct Log {
    pending: Vec<u8>,
    lines: Vec<Line>,
}

impl Log {
    /// Take more UART bytes, read at `at`; a partial last line waits.
    pub(super) fn feed(&mut self, bytes: &[u8], at: f64) {
        self.pending.extend_from_slice(bytes);

        while let Some(end) = self.pending.iter().position(|b| matches!(b, b'\n' | b'\r')) {
            let raw: Vec<u8> = self.pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&raw).trim().to_string();
            if !text.is_empty() {
                self.lines.push(Line { at, text });
            }
        }
        if self.pending.len() > MAX_LINE {
            self.pending.clear();
        }
    }

    /// The position after the last complete line: a mark to search from.
    pub(super) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(super) fn line(&self, index: usize) -> &Line {
        &self.lines[index]
    }

    /// The first line at or after `since` with a record that `matches`, by
    /// line index.
    pub(super) fn find(&self, since: usize, matches: impl Fn(&Record) -> bool) -> Option<usize> {
        (since..self.lines.len()).find(|&i| Record::all(&self.lines[i].text).any(|r| matches(&r)))
    }

    /// How many records at or after `since` match.
    pub(super) fn count(&self, since: usize, matches: impl Fn(&Record) -> bool) -> usize {
        self.lines[since.min(self.lines.len())..]
            .iter()
            .map(|line| Record::all(&line.text).filter(|r| matches(r)).count())
            .sum()
    }

    /// When the last record of any kind arrived.
    pub(super) fn last_record_at(&self) -> Option<f64> {
        self.lines
            .iter()
            .rev()
            .find(|line| Record::parse(&line.text).is_some())
            .map(|line| line.at)
    }
}
