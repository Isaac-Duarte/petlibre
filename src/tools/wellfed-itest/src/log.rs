//! The feeder's UART as a searchable log. Every scenario asserts on the
//! firmware's `PLxxxN` records (`PLAPI1 request slot=0 op=status`), which
//! can sit after SDK noise on the same line; anything else is kept for
//! reading back.
//! Times come from the rig's clock when the bytes were read, so they are as
//! late as the polling interval.

use std::time::Duration;

use wellfed_core::record::{Event, Record};

/// A longer run without a line break is noise, not a record.
const MAX_LINE: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Line {
    /// On the rig's clock.
    pub at: Duration,
    pub text: String,
}

#[derive(Debug, Default)]
pub(super) struct Log {
    pending: Vec<u8>,
    lines: Vec<Line>,
}

impl Log {
    /// Take more UART bytes, read at `at`; a partial last line waits.
    pub(super) fn feed(&mut self, bytes: &[u8], at: Duration) {
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
    pub(super) fn find(&self, since: usize, matches: impl Fn(&Event<'_>) -> bool) -> Option<usize> {
        (since..self.lines.len()).find(|&i| events(&self.lines[i].text).any(|e| matches(&e)))
    }

    /// What `pick` finds first among the events on the line just before
    /// `mark` (an `expect` result): other records may share the line.
    pub(super) fn before<T>(
        &self,
        mark: usize,
        pick: impl Fn(Event<'_>) -> Option<T>,
    ) -> Option<T> {
        events(&self.lines.get(mark.checked_sub(1)?)?.text).find_map(pick)
    }

    /// How many records at or after `since` match.
    pub(super) fn count(&self, since: usize, matches: impl Fn(&Event<'_>) -> bool) -> usize {
        self.lines[since.min(self.lines.len())..]
            .iter()
            .map(|line| events(&line.text).filter(|e| matches(e)).count())
            .sum()
    }

    /// The records at or after `since` and before `until` that match.
    pub(super) fn between(
        &self,
        since: usize,
        until: usize,
        matches: impl Fn(&Event<'_>) -> bool,
    ) -> Vec<String> {
        let until = until.min(self.lines.len());
        self.lines[since.min(until)..until]
            .iter()
            .filter(|line| events(&line.text).any(|e| matches(&e)))
            .map(|line| line.text.clone())
            .collect()
    }

    /// When the last record of any kind arrived.
    pub(super) fn last_record_at(&self) -> Option<Duration> {
        self.lines
            .iter()
            .rev()
            .find(|line| Record::parse(&line.text).is_some())
            .map(|line| line.at)
    }
}

/// The events of the records on one line, in order.
pub(super) fn events(line: &str) -> impl Iterator<Item = Event<'_>> {
    Record::all(line).filter_map(|record| record.event())
}
