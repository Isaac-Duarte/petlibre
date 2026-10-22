//! The firmware's UART records, as the host tools read them back.
//!
//! Every subsystem reports on the UART as `PL<tag><n> <first> key=value …`
//! lines (`PLMQT1 request op=ping ok=1`). SDK output shares the port, so a
//! record can start mid-line or follow another one on the same line. The
//! firmware writes the bodies with `core::fmt` (each module's `describe`);
//! the bench reads them with this parser, and `wellfed-amebad`'s record
//! tests run the `describe` functions through it, so a renamed field fails
//! the host tests rather than a half-hour bench run.

/// The tags the host tools act on.
pub mod tag {
    /// Startup: `start build=… bank=…`, `result diagnostic=…`.
    pub const START: &str = "PLFW1";
    /// KM0's boot trial: `confirmed=0 boot=N limit=3`, `give_way=1`.
    pub const TRIAL: &str = "PLTRL1";
    /// FW11 confirmation: `confirmed=1 healthy_ms=…`.
    pub const CONFIRM: &str = "PLCNF1";
    /// FW10 offer check: `offer=1 same=1`, `give_way=1 version=…`.
    pub const WATCH: &str = "PLWCH1";
    /// FW10 recovery-role updater: `manifest`, `progress sectors=N/T`,
    /// `activated`, `error stage=…`.
    pub const UPDATE: &str = "PLOTA1";
    /// FW08 transport: `connect`, `ready`, `request op=… ok=…`, `closed`.
    pub const TRANSPORT: &str = "PLMQT1";
    /// FW15/FW25 settings writer: `saved generation=…`, `net saved stored=…`.
    pub const SETTINGS: &str = "PLCFG1";
    /// FW21 wall clock: `sync ok=…`.
    pub const CLOCK: &str = "PLTIM1";
}

/// One record: its tag (`PLMQT1`), first word, and `key=value` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub tag: &'a str,
    pub first: &'a str,
    body: &'a str,
}

/// The length of the tag starting `text`, with its space: `PL`, two to four
/// uppercase letters, a digit, then a space (`PLFW1 `, `PLMQT1 `).
fn tag_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes.starts_with(b"PL") {
        return None;
    }
    let letters = bytes[2..]
        .iter()
        .take_while(|b| b.is_ascii_uppercase())
        .count();
    let digit = 2 + letters;
    ((2..=4).contains(&letters)
        && bytes.get(digit).is_some_and(u8::is_ascii_digit)
        && bytes.get(digit + 1) == Some(&b' '))
    .then_some(digit + 2)
}

/// Where the next tag in `text` starts, and its length with the space.
fn next_tag(text: &str) -> Option<(usize, usize)> {
    text.match_indices("PL")
        .find_map(|(at, _)| Some((at, tag_len(&text[at..])?)))
}

impl<'a> Record<'a> {
    /// The first record in `line`.
    pub fn parse(line: &'a str) -> Option<Self> {
        Self::all(line).next()
    }

    /// Every record in `line`, in order; each body ends where the next tag
    /// starts.
    pub fn all(line: &'a str) -> Records<'a> {
        Records { rest: line }
    }

    pub fn field(&self, key: &str) -> Option<&'a str> {
        self.body
            .split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    }

    pub fn number<T: core::str::FromStr>(&self, key: &str) -> Option<T> {
        self.field(key)?.parse().ok()
    }

    /// The text after the tag.
    pub fn body(&self) -> &'a str {
        self.body
    }

    /// The record's text after the tag starts with `prefix` (`net saved`).
    pub fn body_starts(&self, prefix: &str) -> bool {
        self.body.starts_with(prefix)
    }

    /// The tag matches and every `key=value` pair is present.
    pub fn is(&self, tag: &str, fields: &[(&str, &str)]) -> bool {
        self.tag == tag && fields.iter().all(|(k, v)| self.field(k) == Some(*v))
    }
}

/// The records of one line ([`Record::all`]).
pub struct Records<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Records<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Record<'a>> {
        let (at, len) = next_tag(self.rest)?;
        let tag = &self.rest[at..at + len - 1];
        let after = &self.rest[at + len..];
        let end = next_tag(after).map_or(after.len(), |(next, _)| next);

        let body = after[..end].trim();
        self.rest = &after[end..];
        Some(Record {
            tag,
            first: body.split_whitespace().next().unwrap_or(""),
            body,
        })
    }
}
