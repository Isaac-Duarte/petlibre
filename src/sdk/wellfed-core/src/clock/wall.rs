//! FW21 wall clock: UTC from the local server, carried by the uptime clock.
//!
//! The feeder has no RTC backup and no route to the internet, so the clock is
//! invalid at every boot until the server sends a sync. Between syncs the time
//! is the last sync plus elapsed uptime. The uptime clock ran a steady 735 ppm
//! slow against the server on the research unit (two 10-minute resyncs,
//! October 22), so each reading carries an uncertainty that grows by
//! `DRIFT_PPM`, and the clock reads invalid once it passes
//! `MAX_UNCERTAINTY_MS` (about 8 h after a sync), or after any fault.
//!
//! Time zones stay on the server: each sync carries the current UTC offset
//! and the next offset change, so local time is right across one DST change
//! without a zone database here.

/// Earliest accepted sync (2026-01-01T00:00:00Z); an earlier one is a bad
/// server clock, not a time.
pub const MIN_UNIX_MS: u64 = 1_767_225_600_000;
/// Latest accepted sync (2100-01-01T00:00:00Z).
pub const MAX_UNIX_MS: u64 = 4_102_444_800_000;
/// UTC offsets run from -12 h to +14 h.
pub const MIN_OFFSET_S: i32 = -12 * 3600;
pub const MAX_OFFSET_S: i32 = 14 * 3600;
/// Bound on uptime drift: the measured 735 ppm with margin for temperature.
pub const DRIFT_PPM: u64 = 2000;
/// Allowance for the sync's own delay on the local network.
pub const SYNC_UNCERTAINTY_MS: u64 = 1000;
/// A clock less certain than this reads invalid.
pub const MAX_UNCERTAINTY_MS: u64 = 60_000;
/// A sync within this of the running estimate is in step, not a jump.
pub const STEP_MS: i64 = 2000;
/// A scheduled occurrence at most this late still runs (FP03 policy).
pub const MISSED_GRACE_MS: u64 = 15 * 60 * 1000;

/// One sync as the server sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sync {
    pub unix_ms: u64,
    /// Current UTC offset of the owner's zone.
    pub offset_s: i32,
    /// When the offset next changes (Unix seconds), or 0 for no known change.
    pub next_change_s: u64,
    pub next_offset_s: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncError {
    Time,
    Offset,
    NextChange,
}

/// How a sync compared with the running clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    /// No valid clock to compare with: the first sync, or after expiry.
    First,
    /// Within `STEP_MS` of the estimate.
    Step(i64),
    /// The clock moved forward (positive) or back (negative) by this much.
    Jumped(i64),
}

/// A valid reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub uncertainty_ms: u64,
}

impl Reading {
    /// Local wall time in milliseconds since the Unix epoch, as if local were UTC.
    pub fn local_ms(&self) -> i64 {
        self.unix_ms as i64 + i64::from(self.offset_s) * 1000
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WallClock {
    anchor: Option<Anchor>,
    syncs: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    uptime_ms: u32,
    sync: Sync,
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
            syncs: 0,
        }
    }

    /// Adopt `sync` received at uptime `now`. A refused sync leaves the clock
    /// as it was.
    pub fn sync(&mut self, now: u32, sync: Sync) -> Result<Jump, SyncError> {
        sync.validate()?;
        let jump = match self.read(now) {
            None => Jump::First,
            Some(reading) => {
                let delta = sync.unix_ms as i64 - reading.unix_ms as i64;
                if delta.abs() <= STEP_MS {
                    Jump::Step(delta)
                } else {
                    Jump::Jumped(delta)
                }
            }
        };
        self.anchor = Some(Anchor {
            uptime_ms: now,
            sync,
        });
        self.syncs = self.syncs.saturating_add(1);
        Ok(jump)
    }

    /// The time at uptime `now`, or `None` before the first sync and once
    /// the uncertainty passes `MAX_UNCERTAINTY_MS`.
    pub fn read(&self, now: u32) -> Option<Reading> {
        let anchor = self.anchor?;
        let elapsed = u64::from(now.wrapping_sub(anchor.uptime_ms));
        if elapsed >= u64::from(crate::HALF_RANGE) {
            return None;
        }
        let uncertainty_ms = SYNC_UNCERTAINTY_MS + elapsed * DRIFT_PPM / 1_000_000;
        if uncertainty_ms > MAX_UNCERTAINTY_MS {
            return None;
        }

        let unix_ms = anchor.sync.unix_ms + elapsed;
        let change = anchor.sync.next_change_s;
        let offset_s = if change != 0 && unix_ms >= change * 1000 {
            anchor.sync.next_offset_s
        } else {
            anchor.sync.offset_s
        };
        Some(Reading {
            unix_ms,
            offset_s,
            uncertainty_ms,
        })
    }

    pub fn syncs(&self) -> u32 {
        self.syncs
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

/// The missed-meal rule, for FP03. `evaluated_ms` is the persisted high-water
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
