//! The desiccant reminder: how many days are left before the hopper's
//! desiccant pack is due for replacement.
//!
//! The settings record keeps the period and the local calendar day of the
//! last replacement, never a count, so the reminder survives any number of
//! restarts and needs no write as days pass. The feeder has no battery-backed
//! clock: after a cold boot it cannot tell the date until a time source
//! syncs, so the count is unknown until then, and a replacement is recorded
//! only with a valid clock rather than on a guessed date.
//!
//! Days are calendar days of the feeder's local time (UTC while it knows no
//! offset), so a daylight change, which makes one day 23 or 25 hours long,
//! never moves the count by a day. A replacement on day D with a period of
//! P days leaves P days that day, and is due (0 left) on day D + P; the count
//! goes on below zero after that, as stock's cloud reported it.

use crate::clock::wall::Reading;

/// The shortest period between replacements, in days.
pub const PERIOD_MIN: u8 = 1;
/// The longest period between replacements, in days.
pub const PERIOD_MAX: u8 = 90;
/// The period until an owner sets one: stock's cloud used 14 days.
pub const PERIOD_DEFAULT: u8 = 14;

const DAY_MS: i64 = 86_400_000;

/// Whether `days` is a period the settings accept.
pub const fn period_valid(days: u8) -> bool {
    days >= PERIOD_MIN && days <= PERIOD_MAX
}

/// The local calendar day of a valid reading: days since 1970-01-01 in
/// local time, or in UTC when the clock knows no offset.
pub fn today(reading: &Reading) -> Option<i32> {
    let local_ms = match reading.local_ms() {
        Some(local_ms) => local_ms,
        None => i64::try_from(reading.unix_ms).ok()?,
    };
    i32::try_from(local_ms.div_euclid(DAY_MS)).ok()
}

/// Where the reminder stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    /// No replacement is recorded: nothing to count from.
    Unset,
    /// The clock is invalid, so today is unknown.
    Unknown,
    /// Days left until the replacement is due: 0 or below is due.
    Remaining(i32),
}

impl Count {
    /// The count for a `period`, the day of the last replacement and today.
    /// A replacement dated after today (the clock moved back since) counts
    /// as made today: never more than the period is left.
    pub fn new(period: u8, reset_day: Option<i32>, today: Option<i32>) -> Self {
        let Some(reset_day) = reset_day else {
            return Self::Unset;
        };
        let Some(today) = today else {
            return Self::Unknown;
        };

        let elapsed = (i64::from(today) - i64::from(reset_day)).max(0);
        let left = i64::from(period) - elapsed;
        Self::Remaining(i32::try_from(left).unwrap_or(i32::MIN))
    }

    /// Whether the replacement is due: `None` while the count is unknown
    /// or nothing is recorded.
    pub const fn due(self) -> Option<bool> {
        match self {
            Self::Remaining(days) => Some(days <= 0),
            Self::Unset | Self::Unknown => None,
        }
    }
}
