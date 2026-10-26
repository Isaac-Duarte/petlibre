//! The owner's time zone as a POSIX TZ rule (`MST7MDT,M3.2.0,M11.1.0`).
//!
//! The feeder holds no zone database. A client turns the owner's IANA zone
//! into the rule its TZif file ends with (RFC 8536's footer), which states
//! the zone's offsets and when daylight time starts and ends every year, so
//! the feeder keeps local time across every change without another sync.
//! A rule describes the zone's current law only: a government changing its
//! rules needs the client to send the new one.
//!
//! The text is kept as sent, for the time view and the store; [`Rule`] is
//! its parsed form. Offsets here are east-positive seconds, the opposite of
//! the POSIX text's sign.

/// The longest rule text kept: every footer in the tzdb fits
/// (`wellfed-client`'s rule test checks each zone).
pub const TEXT_MAX: usize = 48;

const HOUR: i32 = 3600;
/// RFC 8536 lets a transition's time run from -167 h to +167 h.
const TIME_LIMIT: i32 = 167 * HOUR;
/// A zone's offset stays within a day of UTC.
const OFFSET_LIMIT: i32 = 24 * HOUR;
/// POSIX's default transition time.
const DEFAULT_TIME: i32 = 2 * HOUR;

/// A parsed rule: the standard offset and, if the zone keeps daylight time,
/// its offset and yearly transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// East of UTC, in seconds.
    std_offset_s: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dst {
    offset_s: i32,
    /// In local standard time.
    start: Transition,
    /// In local daylight time.
    end: Transition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transition {
    date: Date,
    /// Seconds after the date's local midnight.
    time_s: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Date {
    /// `Jn`: day 1-365, never counting February 29.
    NoLeap(u16),
    /// `n`: day 0-365, counting February 29.
    Leap(u16),
    /// `Mm.w.d`: weekday `d` (0 Sunday) of week `w` (5 the last) of month `m`.
    Month { month: u8, week: u8, weekday: u8 },
}

/// The rule text failed to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;

/// A rule's text as sent, and its parsed form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    len: u8,
    text: [u8; TEXT_MAX],
    rule: Rule,
}

impl Zone {
    /// Parse `text`; it must be a whole POSIX TZ rule of at most
    /// [`TEXT_MAX`] bytes. Daylight time without transition dates is
    /// refused: POSIX leaves its default to the implementation.
    pub fn parse(text: &[u8]) -> Result<Self, Invalid> {
        if text.len() > TEXT_MAX {
            return Err(Invalid);
        }
        let rule = Parser { text, at: 0 }.rule()?;
        let mut stored = [0; TEXT_MAX];
        stored[..text.len()].copy_from_slice(text);
        Ok(Self {
            len: u8::try_from(text.len()).map_err(|_| Invalid)?,
            text: stored,
            rule,
        })
    }

    pub fn text(&self) -> &[u8] {
        &self.text[..usize::from(self.len)]
    }

    pub const fn rule(&self) -> Rule {
        self.rule
    }
}

impl Rule {
    /// UTC itself.
    pub const UTC: Self = Self {
        std_offset_s: 0,
        dst: None,
    };

    /// The offset east of UTC in force at `unix_s`.
    pub fn offset_at(&self, unix_s: i64) -> i32 {
        let Some(dst) = self.dst else {
            return self.std_offset_s;
        };

        // The year as standard local time sees it; transitions never sit
        // within a day of New Year, so the neighbouring year's never apply.
        let local_days = (unix_s + i64::from(self.std_offset_s)).div_euclid(86_400);
        let (year, _, _) = civil_from_days(local_days);
        let start = dst.start.utc_s(year, self.std_offset_s);
        let end = dst.end.utc_s(year, dst.offset_s);

        let in_dst = if start < end {
            start <= unix_s && unix_s < end
        } else {
            // Southern hemisphere: daylight time spans New Year.
            !(end <= unix_s && unix_s < start)
        };
        if in_dst {
            dst.offset_s
        } else {
            self.std_offset_s
        }
    }

    /// The first instant after `unix_s` at which the offset changes, and
    /// the offset from then, searched across the next two years.
    pub fn next_change(&self, unix_s: i64) -> Option<(i64, i32)> {
        let dst = self.dst?;
        let local_days = (unix_s + i64::from(self.std_offset_s)).div_euclid(86_400);
        let (year, _, _) = civil_from_days(local_days);
        let now = self.offset_at(unix_s);

        let mut best: Option<i64> = None;
        for y in year..=year + 1 {
            for at in [
                dst.start.utc_s(y, self.std_offset_s),
                dst.end.utc_s(y, dst.offset_s),
            ] {
                if at > unix_s && self.offset_at(at) != now && best.is_none_or(|b| at < b) {
                    best = Some(at);
                }
            }
        }
        best.map(|at| (at, self.offset_at(at)))
    }
}

impl Transition {
    /// The transition's instant in `year`, for local time `offset_s` east.
    fn utc_s(&self, year: i64, offset_s: i32) -> i64 {
        let day = match self.date {
            Date::NoLeap(n) => {
                let n = i64::from(n);
                let leap_shift = i64::from(is_leap(year) && n >= 60);
                days_from_civil(year, 1, 1) + n - 1 + leap_shift
            }
            Date::Leap(n) => days_from_civil(year, 1, 1) + i64::from(n),
            Date::Month {
                month,
                week,
                weekday,
            } => {
                let first = days_from_civil(year, month, 1);
                let first_weekday = (first + 4).rem_euclid(7);
                let mut day = first + (i64::from(weekday) - first_weekday).rem_euclid(7);
                day += 7 * (i64::from(week) - 1);
                // Week 5 is the last such weekday, which may be the fourth.
                let next_month = if month == 12 {
                    days_from_civil(year + 1, 1, 1)
                } else {
                    days_from_civil(year, month + 1, 1)
                };
                while day >= next_month {
                    day -= 7;
                }
                day
            }
        };
        day * 86_400 + i64::from(self.time_s) - i64::from(offset_s)
    }
}

const fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's
/// algorithm).
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let month = i64::from(month);
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The offsets the wall clock and the device API accept, -12 h to +14 h.
fn in_range(offset_s: i32) -> Result<i32, Invalid> {
    if (super::wall::MIN_OFFSET_S..=super::wall::MAX_OFFSET_S).contains(&offset_s) {
        Ok(offset_s)
    } else {
        Err(Invalid)
    }
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn rule(mut self) -> Result<Rule, Invalid> {
        self.name()?;
        let std_offset_s = in_range(-self.offset(OFFSET_LIMIT)?)?;
        if self.done() {
            return Ok(Rule {
                std_offset_s,
                dst: None,
            });
        }

        self.name()?;
        let offset_s = in_range(if self.peek() == Some(b',') {
            std_offset_s + HOUR
        } else {
            -self.offset(OFFSET_LIMIT)?
        })?;
        self.expect(b',')?;
        let start = self.transition()?;
        self.expect(b',')?;
        let end = self.transition()?;
        if !self.done() {
            return Err(Invalid);
        }
        Ok(Rule {
            std_offset_s,
            dst: Some(Dst {
                offset_s,
                start,
                end,
            }),
        })
    }

    fn done(&self) -> bool {
        self.at == self.text.len()
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn expect(&mut self, byte: u8) -> Result<(), Invalid> {
        if self.bump() == Some(byte) {
            Ok(())
        } else {
            Err(Invalid)
        }
    }

    /// An abbreviation: three or more letters, or `<...>` of letters,
    /// digits and signs.
    fn name(&mut self) -> Result<(), Invalid> {
        let start = self.at;
        if self.peek() == Some(b'<') {
            self.at += 1;
            let inner = self.at;
            while self
                .peek()
                .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-')
            {
                self.at += 1;
            }
            let len = self.at - inner;
            self.expect(b'>')?;
            return if len >= 3 { Ok(()) } else { Err(Invalid) };
        }
        while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.at += 1;
        }
        if self.at - start >= 3 {
            Ok(())
        } else {
            Err(Invalid)
        }
    }

    /// `[+-]h[h][:mm[:ss]]` in seconds, as written (west-positive for an
    /// offset).
    fn offset(&mut self, limit: i32) -> Result<i32, Invalid> {
        let sign = match self.peek() {
            Some(b'-') => {
                self.at += 1;
                -1
            }
            Some(b'+') => {
                self.at += 1;
                1
            }
            _ => 1,
        };
        let hours = self.number(3)?;
        let mut seconds = hours * HOUR;
        if self.peek() == Some(b':') {
            self.at += 1;
            let minutes = self.number(2)?;
            if minutes > 59 {
                return Err(Invalid);
            }
            seconds += minutes * 60;
            if self.peek() == Some(b':') {
                self.at += 1;
                let secs = self.number(2)?;
                if secs > 59 {
                    return Err(Invalid);
                }
                seconds += secs;
            }
        }
        if seconds > limit {
            return Err(Invalid);
        }
        Ok(sign * seconds)
    }

    /// One to `digits` decimal digits.
    fn number(&mut self, digits: usize) -> Result<i32, Invalid> {
        let start = self.at;
        let mut value: i32 = 0;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) && self.at - start < digits {
            let digit = self.bump().ok_or(Invalid)?;
            value = value * 10 + i32::from(digit - b'0');
        }
        if self.at == start {
            Err(Invalid)
        } else {
            Ok(value)
        }
    }

    fn transition(&mut self) -> Result<Transition, Invalid> {
        let date = match self.peek() {
            Some(b'J') => {
                self.at += 1;
                let n = self.number(3)?;
                if !(1..=365).contains(&n) {
                    return Err(Invalid);
                }
                Date::NoLeap(u16::try_from(n).map_err(|_| Invalid)?)
            }
            Some(b'M') => {
                self.at += 1;
                let month = self.number(2)?;
                self.expect(b'.')?;
                let week = self.number(1)?;
                self.expect(b'.')?;
                let weekday = self.number(1)?;
                if !(1..=12).contains(&month) || !(1..=5).contains(&week) || weekday > 6 {
                    return Err(Invalid);
                }
                Date::Month {
                    month: u8::try_from(month).map_err(|_| Invalid)?,
                    week: u8::try_from(week).map_err(|_| Invalid)?,
                    weekday: u8::try_from(weekday).map_err(|_| Invalid)?,
                }
            }
            _ => {
                let n = self.number(3)?;
                if n > 365 {
                    return Err(Invalid);
                }
                Date::Leap(u16::try_from(n).map_err(|_| Invalid)?)
            }
        };
        let time_s = if self.peek() == Some(b'/') {
            self.at += 1;
            self.offset(TIME_LIMIT)?
        } else {
            DEFAULT_TIME
        };
        Ok(Transition { date, time_s })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_round_trip() {
        for days in [-1, 0, 59, 365, 11_016, 20_000, 47_482] {
            let (y, m, d) = civil_from_days(days);
            let m = u8::try_from(m).unwrap();
            let d = u8::try_from(d).unwrap();
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}
