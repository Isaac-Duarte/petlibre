//! Feeding plans: what a plan says, and when its occurrences fall.
//!
//! A plan feeds `portions` at minute `minute` of the local day, on the
//! weekdays it lists, or once at its next occurrence when it lists none.
//! The feeder keeps [`PLANS`] of them in fixed slots, as stock keeps at most
//! ten; the slot is the plan's identity.
//!
//! **Local time to UTC.** An occurrence is the earliest UTC instant whose
//! local time is the plan's minute on that date ([`utc_of`]). A minute a
//! spring-forward change skips runs as far after the change as it would
//! have been (02:30 runs at 03:30 when the clock jumps from 02:00), and a
//! minute a fall-back change repeats runs on its first pass only: the rule
//! RFC 5545 and Temporal's "compatible" disambiguation use. Offsets come
//! from the clock ([`crate::clock::wall::WallClock::offset_at`]): the zone's
//! rule, or a client sync's offsets until the next change it named.
//!
//! What an occurrence does (run, report as missed, or nothing) is the
//! missed-meal rule, [`crate::clock::wall::occurrence`], against a mark per
//! plan: the loop that keeps the marks lives in the product.

/// Plan slots a feeder keeps, as stock keeps at most ten plans.
pub const PLANS: usize = 10;
/// The most portions one plan asks for: the most one feed may.
pub const PORTIONS_MAX: u8 = 48;

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;
/// Minutes in a local day: a plan's minute is below it.
pub const DAY_MINUTES: u16 = 24 * 60;
/// Local dates a weekly plan's latest occurrence can lie behind today.
const WEEK_BACK: i64 = 7;

/// The weekdays a plan runs on: bit 0 Monday to bit 6 Sunday. None set is
/// a plan that runs once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Weekdays(pub u8);

impl Weekdays {
    pub const ONCE: Self = Self(0);
    pub const EVERY_DAY: Self = Self(0x7f);

    pub const fn once(self) -> bool {
        self.0 == 0
    }

    pub const fn valid(self) -> bool {
        self.0 & !Self::EVERY_DAY.0 == 0
    }

    /// Whether the plan runs on `weekday` (0 Monday to 6 Sunday).
    pub const fn contains(self, weekday: u8) -> bool {
        weekday < 7 && self.0 & (1 << weekday) != 0
    }
}

/// One feeding plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Minute of the local day, below [`DAY_MINUTES`].
    pub minute: u16,
    /// 1 to [`PORTIONS_MAX`].
    pub portions: u8,
    pub days: Weekdays,
    pub enabled: bool,
}

impl Plan {
    /// Whether every field is one the feeder accepts. Stock checked none
    /// (a plan at "99:99" stored as given); this one refuses them.
    pub const fn valid(&self) -> bool {
        self.minute < DAY_MINUTES
            && self.portions >= 1
            && self.portions <= PORTIONS_MAX
            && self.days.valid()
    }

    /// Whether `other` keeps this plan's occurrences: the same minute and
    /// days. A change to either starts the plan afresh from its save.
    pub const fn same_times(&self, other: &Self) -> bool {
        self.minute == other.minute && self.days.0 == other.days.0
    }
}

/// The plan in each slot.
pub type Table = [Option<Plan>; PLANS];

/// Whether every plan in `table` is valid.
pub fn table_valid(table: &Table) -> bool {
    table.iter().flatten().all(Plan::valid)
}

/// The weekday of the local date `day` (days since 1970-01-01, a
/// Thursday): 0 Monday to 6 Sunday.
pub fn weekday(day: i64) -> u8 {
    u8::try_from((day + 3).rem_euclid(7)).unwrap_or(0)
}

/// The UTC instant (Unix milliseconds) of the local wall time `local_ms`,
/// by the offsets `offset_at` gives (east-positive seconds at a Unix
/// millisecond); `None` when it gives none.
///
/// The offsets a day either side bracket any change near `local_ms`, since
/// a zone changes at most once in two days. Each that maps the local time
/// back onto itself is a real instant, and the earlier wins (a repeated
/// minute's first pass). None does in a skipped minute, which then runs at
/// the earlier offset's instant: as far past the change as it was meant
/// to be past the minute before.
pub fn utc_of(local_ms: i64, offset_at: impl Fn(u64) -> Option<i32>) -> Option<u64> {
    let at = |ms: i64| offset_at(u64::try_from(ms).ok()?).map(|s| i64::from(s) * 1000);
    let before = at(local_ms - DAY_MS)?;
    let after = at(local_ms + DAY_MS)?;

    let fits = |offset: i64| at(local_ms - offset) == Some(offset);
    let utc = match (fits(before), fits(after)) {
        (true, true) => (local_ms - before).min(local_ms - after),
        (true, false) => local_ms - before,
        (false, true) => local_ms - after,
        (false, false) => local_ms - before,
    };
    u64::try_from(utc).ok()
}

/// The latest occurrence of `plan` at or before `now_ms` (Unix
/// milliseconds), whether or not the plan is enabled; `None` without
/// offsets. A plan that runs once occurs every day until it runs.
pub fn latest_due(plan: &Plan, now_ms: u64, offset_at: impl Fn(u64) -> Option<i32>) -> Option<u64> {
    let now = i64::try_from(now_ms).ok()?;
    let local_now = now + i64::from(offset_at(now_ms)?) * 1000;
    let today = local_now.div_euclid(DAY_MS);
    let minute_ms = i64::from(plan.minute) * MINUTE_MS;

    // A repeated or skipped hour can put today's occurrence after now (or
    // tomorrow's, by the date, before it), so dates are tried from
    // tomorrow back.
    for day in (today - WEEK_BACK..=today + 1).rev() {
        if !plan.days.once() && !plan.days.contains(weekday(day)) {
            continue;
        }
        let due = utc_of(day * DAY_MS + minute_ms, &offset_at)?;
        if due <= now_ms {
            return Some(due);
        }
    }
    None
}
