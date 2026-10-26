//! Wall clock: UTC from fixes, carried by the uptime clock between them.
//!
//! The feeder has no battery-backed clock, so the wall clock is invalid at
//! every boot until a fix arrives: a sync from a client's session, or an
//! NTP answer. Between fixes the time is the last estimate plus elapsed
//! uptime, corrected by the uptime clock's learned rate.
//!
//! **Error bound.** Every estimate carries how far off it could be: the
//! last fix's own error plus the rate's bound times the time since. Before
//! a rate is learned the bound is [`UNLEARNED_PPM`] (the research unit's
//! uptime clock ran a steady 735 ppm slow, two 10-minute resyncs, October
//! 22). Once two fixes far enough apart measure the rate to within
//! [`LEARN_MAX_PPM`], the correction applies and the bound drops to
//! [`LEARNED_FLOOR_PPM`], or to how far successive measurements disagree
//! if that is more. The clock reads invalid once the bound passes
//! [`MAX_ERROR_MS`]: about 16 h after a fix unlearned, about 14 days
//! learned.
//!
//! **Who may move it.** An owner's sync replaces the estimate whatever it
//! says: the owner is the authority on the time. Any other fix (NTP, or a
//! member's or the server's sync) may only narrow it: the new estimate is
//! the overlap of the two error intervals, and a fix whose interval misses
//! the clock's is refused. So a lying NTP server or a member cannot step a
//! valid clock, and repeated fixes cannot walk it faster than its own
//! error grows. With no valid clock any fix sets it.
//!
//! **Local time.** The zone setting's rule ([`Rule`]) gives the offset at
//! any instant. Without one, a client sync's offset and next change stand
//! in, as before the setting existed, correct across one change.

use super::zone::Rule;

/// Earliest accepted fix (2026-01-01T00:00:00Z); an earlier one is a bad
/// source clock, not a time.
pub const MIN_UNIX_MS: u64 = 1_767_225_600_000;
/// Latest accepted fix (2100-01-01T00:00:00Z).
pub const MAX_UNIX_MS: u64 = 4_102_444_800_000;
/// UTC offsets run from -12 h to +14 h.
pub const MIN_OFFSET_S: i32 = -12 * 3600;
pub const MAX_OFFSET_S: i32 = 14 * 3600;
/// The rate bound before one is learned: the measured 735 ppm with margin
/// for temperature and for another unit.
pub const UNLEARNED_PPM: u32 = 2000;
/// The least rate bound once learned: what a learned correction leaves to
/// temperature and ageing.
pub const LEARNED_FLOOR_PPM: u32 = 100;
/// A rate is learned only from a baseline measuring it this well.
pub const LEARN_MAX_PPM: u32 = 50;
/// A measured rate beyond this is a bad fix, not the oscillator.
pub const RATE_LIMIT_PPM: i32 = 5000;
/// Allowance for a client sync's own delay on the local network.
pub const SYNC_ERROR_MS: u32 = 1000;
/// A clock that could be further off than this reads invalid.
pub const MAX_ERROR_MS: u64 = 120_000;
/// A fix within this of the running estimate is in step, not a jump.
pub const STEP_MS: i64 = 2000;
/// A scheduled occurrence at most this late still runs (the feeding-plan policy).
pub const MISSED_GRACE_MS: u64 = 15 * 60 * 1000;

/// Parts per billion. The arithmetic stays in `i64`: an elapsed time is
/// under 2^31 ms and a rate under 2^23 ppb, so no product overflows, and
/// the KM4 has no 128-bit divide.
const PPB: i64 = 1_000_000_000;

/// One sync as a client sends it: the time and the zone's offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sync {
    pub unix_ms: u64,
    /// Current UTC offset of the owner's zone.
    pub offset_s: i32,
    /// When the offset next changes (Unix seconds), or 0 for no known change.
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

/// Who a fix came from, which decides how far it may move the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Origin {
    /// An owner's session: steps the clock freely.
    Owner = 0,
    /// A member's or the server's session: only narrows a valid clock.
    Client = 1,
    /// An NTP answer: only narrows a valid clock.
    Ntp = 2,
}

impl Origin {
    /// The variant's number, as records print it.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// One measurement of the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fix {
    pub unix_ms: u64,
    /// How far off the source could be, delay included.
    pub error_ms: u32,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncError {
    Time,
    Offset,
    NextChange,
    /// A fix that may not step the clock disagreed with it beyond both
    /// error bounds.
    Bounded,
}

/// How a fix compared with the running clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    /// No valid clock to compare with: the first fix, or after expiry.
    First,
    /// The clock moved by at most `STEP_MS`.
    Step(i64),
    /// The clock moved forward (positive) or back (negative) by this much.
    Jumped(i64),
}

/// A valid reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub unix_ms: u64,
    /// East of UTC; `None` when neither a zone setting nor a client sync
    /// has given one.
    pub offset_s: Option<i32>,
    /// How far off the reading could be.
    pub error_ms: u64,
}

impl Reading {
    /// Local wall time in milliseconds since the Unix epoch, as if local
    /// were UTC; `None` without an offset.
    pub fn local_ms(&self) -> Option<i64> {
        let unix = i64::try_from(self.unix_ms).ok()?;
        Some(unix + i64::from(self.offset_s?) * 1000)
    }
}

/// Everything the time view reports about the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// `None` while invalid: the feeder needs the time.
    pub reading: Option<Reading>,
    pub syncs: u32,
    /// The learned correction, parts per billion of uptime (positive: the
    /// uptime clock runs slow).
    pub rate_ppb: i32,
    /// How far the true rate could be from it.
    pub rate_bound_ppb: u32,
    pub learned: bool,
    /// The last fix's origin and when it came (uptime).
    pub last: Option<(Origin, u32)>,
}

/// A point of the estimate, in microseconds: an error bound rounded up to
/// the millisecond at every narrowing would let fixes walk the clock a
/// millisecond each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    uptime_ms: u32,
    unix_us: u64,
    error_us: u64,
}

/// A client sync's offsets, used while no zone is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hint {
    offset_s: i32,
    next_change_s: u64,
    next_offset_s: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallClock {
    anchor: Option<Anchor>,
    /// The first fix since the clock was last set from nothing or stepped:
    /// the rate is measured across it.
    base: Option<Anchor>,
    rate_ppb: i32,
    rate_bound_ppb: u32,
    learned: bool,
    hint: Option<Hint>,
    rule: Option<Rule>,
    syncs: u32,
    last: Option<(Origin, u32)>,
}

impl Default for WallClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Sync {
    fn validate(&self) -> Result<(), SyncError> {
        if !(MIN_UNIX_MS..MAX_UNIX_MS).contains(&self.unix_ms) {
            return Err(SyncError::Time);
        }
        let offsets = MIN_OFFSET_S..=MAX_OFFSET_S;
        if !offsets.contains(&self.offset_s) || !offsets.contains(&self.next_offset_s) {
            return Err(SyncError::Offset);
        }
        if self.next_change_s != 0 && self.next_change_s.saturating_mul(1000) <= self.unix_ms {
            return Err(SyncError::NextChange);
        }
        Ok(())
    }
}

impl WallClock {
    /// Unset, for a static.
    pub const fn new() -> Self {
        Self {
            anchor: None,
            base: None,
            rate_ppb: 0,
            rate_bound_ppb: UNLEARNED_PPM * 1000,
            learned: false,
            hint: None,
            rule: None,
            syncs: 0,
            last: None,
        }
    }

    /// Adopt a client's sync received at uptime `now`, as `origin` (an
    /// owner's session or another). A refused sync leaves the clock as it
    /// was; an accepted one also leaves its offsets for local time while
    /// no zone is set.
    pub fn sync(&mut self, now: u32, sync: Sync, origin: Origin) -> Result<Jump, SyncError> {
        sync.validate()?;
        let jump = self.fix(
            now,
            Fix {
                unix_ms: sync.unix_ms,
                error_ms: SYNC_ERROR_MS,
                origin,
            },
        )?;
        self.hint = Some(Hint {
            offset_s: sync.offset_s,
            next_change_s: sync.next_change_s,
            next_offset_s: sync.next_offset_s,
        });
        Ok(jump)
    }

    /// Adopt `fix`, measured at uptime `now`.
    pub fn fix(&mut self, now: u32, fix: Fix) -> Result<Jump, SyncError> {
        if !(MIN_UNIX_MS..MAX_UNIX_MS).contains(&fix.unix_ms) {
            return Err(SyncError::Time);
        }
        let point = Anchor {
            uptime_ms: now,
            unix_us: fix.unix_ms * 1000,
            error_us: u64::from(fix.error_ms) * 1000,
        };

        let jump = match self
            .estimate(now)
            .filter(|e| e.error_us <= MAX_ERROR_MS * 1000)
        {
            None => {
                self.anchor = Some(point);
                self.base = Some(point);
                Jump::First
            }
            Some(estimate) => {
                let delta = signed(point.unix_us, estimate.unix_us);
                let consistent = delta.unsigned_abs() <= estimate.error_us + point.error_us;
                if fix.origin == Origin::Owner {
                    self.anchor = Some(point);
                    if consistent {
                        self.learn(point);
                    } else {
                        // The clock was wrong: what it was measured
                        // against says nothing about the rate.
                        self.base = Some(point);
                    }
                    classify(delta)
                } else if consistent {
                    let narrowed = narrow(estimate, point);
                    self.anchor = Some(narrowed);
                    self.learn(point);
                    classify(signed(narrowed.unix_us, estimate.unix_us))
                } else {
                    return Err(SyncError::Bounded);
                }
            }
        };
        self.syncs = self.syncs.saturating_add(1);
        self.last = Some((fix.origin, now));
        Ok(jump)
    }

    /// Measure the rate across the baseline, once it is long enough to do
    /// so within `LEARN_MAX_PPM`.
    fn learn(&mut self, point: Anchor) {
        let Some(base) = self.base else {
            self.base = Some(point);
            return;
        };
        let span = point.uptime_ms.wrapping_sub(base.uptime_ms);
        if span >= crate::HALF_RANGE {
            self.base = Some(point);
            return;
        }
        if span == 0 {
            return;
        }

        let span = i64::from(span);
        let gained = (signed(point.unix_us, base.unix_us) / 1000).saturating_sub(span);
        // Beyond the rate limit before multiplying, which keeps the product
        // in range.
        let limit = i64::from(RATE_LIMIT_PPM) * 1000;
        if gained.unsigned_abs() > span.unsigned_abs() / 100 {
            return;
        }
        let measured = gained * PPB / span;
        let errors_ms = (base.error_us + point.error_us).div_ceil(1000);
        let errors = i64::try_from(errors_ms).unwrap_or(i64::MAX / PPB);
        let uncertainty = errors.saturating_mul(PPB) / span;
        if uncertainty > i64::from(LEARN_MAX_PPM) * 1000 || measured.abs() > limit {
            return;
        }
        let (Ok(measured), Ok(uncertainty)) = (i32::try_from(measured), u32::try_from(uncertainty))
        else {
            return;
        };

        // A rate that moved since the last measurement is evidence of how
        // much it varies: the bound covers that too.
        let moved = if self.learned {
            measured.abs_diff(self.rate_ppb)
        } else {
            0
        };
        self.rate_ppb = measured;
        self.rate_bound_ppb =
            (uncertainty + moved).clamp(LEARNED_FLOOR_PPM * 1000, UNLEARNED_PPM * 1000);
        self.learned = true;
    }

    /// Set or clear the zone's rule.
    pub fn set_rule(&mut self, rule: Option<Rule>) {
        self.rule = rule;
    }

    /// The estimate at uptime `now`, valid or not; `None` before the first
    /// fix or past half the uptime clock's range.
    fn estimate(&self, now: u32) -> Option<Anchor> {
        let anchor = self.anchor?;
        let elapsed = now.wrapping_sub(anchor.uptime_ms);
        if elapsed >= crate::HALF_RANGE {
            return None;
        }

        // Microseconds: ppb of milliseconds is a millionth.
        let elapsed = i64::from(elapsed);
        let corrected = elapsed * 1000 + elapsed * i64::from(self.rate_ppb) / 1_000_000;
        let unix_us = u64::try_from(i64::try_from(anchor.unix_us).ok()? + corrected).ok()?;
        let grown = u64::try_from(elapsed).ok()? * u64::from(self.rate_bound_ppb);
        Some(Anchor {
            uptime_ms: now,
            unix_us,
            error_us: anchor.error_us + grown.div_ceil(1_000_000),
        })
    }

    /// The time at uptime `now`, or `None` before the first fix and once
    /// the error bound passes `MAX_ERROR_MS`.
    pub fn read(&self, now: u32) -> Option<Reading> {
        let estimate = self.estimate(now)?;
        let error_ms = estimate.error_us.div_ceil(1000);
        if error_ms > MAX_ERROR_MS {
            return None;
        }
        let unix_ms = estimate.unix_us / 1000;
        Some(Reading {
            unix_ms,
            offset_s: self.offset_at(unix_ms),
            error_ms,
        })
    }

    fn offset_at(&self, unix_ms: u64) -> Option<i32> {
        let unix_s = i64::try_from(unix_ms / 1000).ok()?;
        if let Some(rule) = self.rule {
            return Some(rule.offset_at(unix_s));
        }
        let hint = self.hint?;
        let change = hint.next_change_s;
        Some(if change != 0 && unix_ms >= change.saturating_mul(1000) {
            hint.next_offset_s
        } else {
            hint.offset_s
        })
    }

    pub fn state(&self, now: u32) -> State {
        State {
            reading: self.read(now),
            syncs: self.syncs,
            rate_ppb: self.rate_ppb,
            rate_bound_ppb: self.rate_bound_ppb,
            learned: self.learned,
            last: self.last,
        }
    }

    pub fn syncs(&self) -> u32 {
        self.syncs
    }
}

fn signed(a: u64, b: u64) -> i64 {
    let (Ok(a), Ok(b)) = (i64::try_from(a), i64::try_from(b)) else {
        return i64::MAX;
    };
    a - b
}

/// How far the clock moved, from microseconds.
fn classify(delta_us: i64) -> Jump {
    let delta = delta_us / 1000;
    if delta.abs() <= STEP_MS {
        Jump::Step(delta)
    } else {
        Jump::Jumped(delta)
    }
}

/// The overlap of two intervals known to overlap: its middle, and its half
/// width as the error.
fn narrow(clock: Anchor, fix: Anchor) -> Anchor {
    let low = clock
        .unix_us
        .saturating_sub(clock.error_us)
        .max(fix.unix_us.saturating_sub(fix.error_us));
    let high = (clock.unix_us + clock.error_us).min(fix.unix_us + fix.error_us);
    let width = high.saturating_sub(low);
    Anchor {
        uptime_ms: fix.uptime_ms,
        unix_us: low + width / 2,
        error_us: width.div_ceil(2),
    }
}

/// What a schedule does with one occurrence due at `due_ms` (UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occurrence {
    /// Not due yet.
    Later,
    /// Already evaluated: never run twice, whatever the clock did since.
    Evaluated,
    Run,
    /// Due more than `MISSED_GRACE_MS` ago; reported, never run.
    Missed,
}

/// The missed-meal rule, for feeding plans. `evaluated_ms` is the persisted high-water
/// mark of time already evaluated (`None` for a new schedule); `now_ms` is a
/// valid reading. Without a valid reading nothing runs.
///
/// - A backward jump re-enters evaluated time, which never runs again.
/// - A forward jump, a restart or a long invalid spell skips time: an
///   occurrence in it runs only if it is at most `MISSED_GRACE_MS` late.
///
/// After evaluating, the caller raises the mark to `max(evaluated, now)`.
pub fn occurrence(due_ms: u64, evaluated_ms: Option<u64>, now_ms: u64) -> Occurrence {
    if evaluated_ms.is_some_and(|mark| due_ms <= mark) {
        Occurrence::Evaluated
    } else if due_ms > now_ms {
        Occurrence::Later
    } else if now_ms - due_ms <= MISSED_GRACE_MS {
        Occurrence::Run
    } else {
        Occurrence::Missed
    }
}
