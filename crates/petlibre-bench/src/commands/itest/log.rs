//! The feeder's UART as a searchable log. Every scenario asserts on the
//! firmware's `PLxxxN` records (`PLMQT1 request op=ping ok=1`), which can sit
//! after SDK noise on the same line; anything else is kept only for the
//! failure tail. Times come from the rig's clock when the bytes were read,
//! so they are as late as the polling interval.

/// A longer run without a line break is noise, not a record.
const MAX_LINE: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: f64,
    pub text: String,
}

/// One `PL…` record: its tag (`PLMQT1`), first word, and `key=value` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub tag: &'a str,
    pub first: &'a str,
    body: &'a str,
}

/// A tag is `PL`, two to four uppercase letters, a digit, then a space
/// (`PLFW1 `, `PLMQT1 `).
fn tag_at(line: &str, at: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let rest = &bytes[at..];
    if !rest.starts_with(b"PL") {
        return None;
    }
    let letters = rest[2..]
        .iter()
        .take_while(|b| b.is_ascii_uppercase())
        .count();
    let digit = 2 + letters;
    ((2..=4).contains(&letters)
        && rest.get(digit).is_some_and(u8::is_ascii_digit)
        && rest.get(digit + 1) == Some(&b' '))
    .then_some(digit + 1)
}

impl<'a> Record<'a> {
    pub fn parse(line: &'a str) -> Option<Self> {
        let (at, len) = line
            .match_indices("PL")
            .find_map(|(at, _)| Some((at, tag_at(line, at)?)))?;
        let body = line[at + len..].trim();
        Some(Self {
            tag: &line[at..at + len],
            first: body.split_whitespace().next().unwrap_or(""),
            body,
        })
    }

    pub fn field(&self, key: &str) -> Option<&'a str> {
        self.body
            .split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    }

    pub fn number<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.field(key)?.parse().ok()
    }

    /// The record's text after the tag starts with `prefix` (`net saved`).
    pub fn body_starts(&self, prefix: &str) -> bool {
        self.body.starts_with(prefix)
    }

    /// The tag matches and every `key=value` pair is present.
    pub fn is(&self, tag: &str, fields: &[(&str, &str)]) -> bool {
        self.tag == tag && fields.iter().all(|(k, v)| self.field(k) == Some(v))
    }
}

#[derive(Debug, Default)]
pub struct Log {
    pending: Vec<u8>,
    lines: Vec<Line>,
}

impl Log {
    /// Take more UART bytes, read at `at`; a partial last line waits.
    pub fn feed(&mut self, bytes: &[u8], at: f64) {
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
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn line(&self, index: usize) -> &Line {
        &self.lines[index]
    }

    /// The first record at or after `since` that `matches`, by line index.
    pub fn find(&self, since: usize, matches: impl Fn(&Record) -> bool) -> Option<usize> {
        (since..self.lines.len())
            .find(|&i| Record::parse(&self.lines[i].text).is_some_and(|record| matches(&record)))
    }

    /// How many records at or after `since` match.
    pub fn count(&self, since: usize, matches: impl Fn(&Record) -> bool) -> usize {
        self.lines[since.min(self.lines.len())..]
            .iter()
            .filter(|line| Record::parse(&line.text).is_some_and(|record| matches(&record)))
            .count()
    }

    /// When the last record of any kind arrived.
    pub fn last_record_at(&self) -> Option<f64> {
        self.lines
            .iter()
            .rev()
            .find(|line| Record::parse(&line.text).is_some())
            .map(|line| line.at)
    }

    /// The last `count` lines, for a failure report.
    pub fn tail(&self, count: usize) -> &[Line] {
        &self.lines[self.lines.len().saturating_sub(count)..]
    }
}
