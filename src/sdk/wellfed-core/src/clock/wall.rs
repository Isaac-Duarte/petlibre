//! Wall clock: UTC from fixes, carried by the uptime clock between them.
//!
//! The feeder has no battery-backed clock, so after a power-on the wall
//! clock is invalid until a fix arrives: a sync from a client's session, or
//! an NTP answer. Between fixes the time is carried by uptime, corrected by
//! the uptime clock's learned rate. Across a warm reset it is carried by
//! the always-on RTC instead ([`WallClock::carry`], [`WallClock::resume`]).
//!
//! **The trusted interval.** The clock keeps an anchor that only a trusted
//! fix sets: an owner's sync, or any fix while the clock is invalid (the
//! first, or after expiry). From the anchor the true time lies within the
//! anchor's error plus the rate bound times the time since: [`UNLEARNED_PPM`]
//! before a rate is learned, and once two trusted fixes far enough apart
//! measure the rate to within [`LEARN_MAX_PPM`], the correction applies and
//! the bound drops to [`LEARNED_FLOOR_PPM`], or to how far successive
//! measurements disagree if that is more. The rate is learned from trusted
//! fixes only, so a spoofed answer cannot poison it: a rate measured from
//! fixes that only had to fall inside the trusted interval could be off by
//! that interval's width over the span, which is the unlearned bound again,
//! so it could move the reading but never tighten the bound.
//!
//! **Carried across a warm reset.** The reset keeps the KM0 RTC counting
//! seconds but loses uptime, so the trusted interval and the reading are
//! saved at an RTC second and, after a watchdog or software reset, resumed
//! with their error grown by the RTC's elapsed seconds at [`RTC_PPM`] plus
//! its one-second resolution. Nothing is resumed after a power-on (the RTC
//! restarts at 0 then), from a record that fails its check, across an RTC
//! that went backwards, or after more than [`CARRY_MAX_S`]. The learned
//! rate is not carried: the next boot learns it again.
//!
//! **The reading.** Any other fix (NTP, or a member's or the server's
//! sync) must fall inside the trusted interval, or it is refused; inside,
//! it moves the reading's centre to the overlap of the two intervals. The
//! reading's error is never less than the trusted interval allows: the
//! centre's distance from the anchor's estimate plus the anchor's error. So
//! an untrusted fix can improve where the clock reads within what it knows,
//! but cannot make it claim more than it knows, and cannot walk it outside
//! the trusted interval, which a later trusted fix resets.
//!
//! **Validity.** The clock reads invalid once its error passes
//! [`MAX_ERROR_MS`]: about 6.6 h after a trusted fix unlearned, about 22 h
//! learned. Then the next fix of any origin sets it again. A run of
//! [`DISJOINT_LIMIT`] NTP answers that all miss the trusted interval means
//! the anchor or the learned rate is wrong (or the server is): the clock
//! forgets both and reads invalid, so it needs the time rather than
//! staying silently valid. An owner's sync that disagrees with the clock
//! also forgets the learned rate.
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
/// The rate bound before one is learned. The uptime clock has been
/// measured 735 ppm slow (two 10-minute resyncs) and 2,060 ppm slow (one
/// 17-minute NTP poll, four days later, another build); this is more than
/// twice the worst. The cost is the validity window, `MAX_ERROR_MS` over
/// the bound: about 6.6 h after a trusted fix.
pub const UNLEARNED_PPM: u32 = 5000;
/// The least rate bound once learned. The same unit's rate moved 1,325 ppm
/// between two measurements, so a learned correction is not trusted any
/// tighter than that until the bench shows how much it moves within a
/// boot: about 22 h of validity.
pub const LEARNED_FLOOR_PPM: u32 = 1500;
/// A rate is learned only from a baseline measuring it this well.
pub const LEARN_MAX_PPM: u32 = 50;
/// A measured rate beyond this (1 %) is a bad fix, not the oscillator.
pub const RATE_LIMIT_PPM: i32 = 10_000;
/// The rate bound across a warm reset, carried by the KM0 RTC's 32 kHz
/// clock. It kept 1,613 s against the host's 1,613.1 s (physical, at its
/// one-second resolution, so within about 620 ppm); the gap is seconds, so
/// a wide margin costs little.
pub const RTC_PPM: u32 = 10_000;
/// A saved clock older than this, in RTC seconds, is not resumed. A
/// running clock is saved every few seconds, so a longer gap means a boot
/// in between that did not save it, or an RTC that wrapped.
pub const CARRY_MAX_S: u32 = 600;
/// Words in a saved clock ([`WallClock::carry`]).
pub const CARRY_WORDS: usize = 16;
/// Allowance for a client sync's own delay on the local network.
pub const SYNC_ERROR_MS: u32 = 1000;
/// A clock that could be further off than this reads invalid.
pub const MAX_ERROR_MS: u64 = 120_000;
/// A fix within this of the running estimate is in step, not a jump.
pub const STEP_MS: i64 = 2000;
/// NTP answers in a row that miss the trusted interval before the clock
/// gives up on it.
pub const DISJOINT_LIMIT: u8 = 3;
/// A scheduled occurrence at most this late still runs (the feeding-plan policy).
pub const MISSED_GRACE_MS: u64 = 15 * 60 * 1000;

/// Parts per billion. The arithmetic stays in `i64`: an elapsed time is
/// under 2^31 ms and a rate under 2^24 ppb, so no product overflows, and
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
    /// A member's or the server's session: only moves a valid clock within
    /// its trusted interval.
    Client = 1,
    /// An NTP answer: as a client's.
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
    /// A fix that may not step the clock fell outside its trusted interval.
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

/// How this boot began, which decides whether a saved clock may resume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    /// A power-on, brownout or deep-sleep wake: the RTC restarted with the
    /// supply, so its count says nothing about the time since the save.
    Cold,
    /// A watchdog or software reset: the RTC kept counting through it.
    Warm,
}

/// Why a saved clock was not resumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NoResume {
    /// The clock already has the time.
    Set = 0,
    Cold = 1,
    /// No saved clock: never saved, cleared, or memory a power-on left.
    Empty = 2,
    /// A record whose check, version or values are wrong.
    Corrupt = 3,
    /// The RTC could not be read as a count.
    Rtc = 4,
    /// The RTC reads earlier than the save: it restarted.
    Backwards = 5,
    /// More than `CARRY_MAX_S` since the save.
    Stale = 6,
    /// The grown error passes `MAX_ERROR_MS`.
    Expired = 7,
}

impl NoResume {
    /// The variant's number, as records print it.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// A saved clock resumed: the RTC seconds since the save, and the error the
/// reading now claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resumed {
    pub gap_s: u32,
    pub error_ms: u64,
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

/// A point of the clock, in microseconds: an error rounded up to the
/// millisecond at every fix would let fixes walk it a millisecond each.
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
    /// The last trusted fix.
    trust: Option<Anchor>,
    /// Where the clock reads: the trusted fix, or an untrusted fix's
    /// overlap with the trusted interval. Its own error is never reported.
    centre: Option<Anchor>,
    /// The first trusted fix since the rate was last forgotten: the rate
    /// is measured across it.
    base: Option<Anchor>,
    rate_ppb: i32,
    rate_bound_ppb: u32,
    learned: bool,
    /// NTP answers in a row outside the trusted interval.
    disjoint: u8,
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
            trust: None,
            centre: None,
            base: None,
            rate_ppb: 0,
            rate_bound_ppb: UNLEARNED_PPM * 1000,
            learned: false,
            disjoint: 0,
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

        let jump = match (self.current(now), fix.origin) {
            // Nothing valid to weigh it against: any fix is trusted.
            (None, _) => {
                self.trusted(point);
                self.learn(point);
                Jump::First
            }
            (Some((_, centre, error_us)), Origin::Owner) => {
                let delta = signed(point.unix_us, centre.unix_us);
                let consistent = delta.unsigned_abs() <= error_us + point.error_us;
                self.trusted(point);
                if consistent {
                    self.learn(point);
                } else {
                    // The clock was wrong, so its rate may be: what it was
                    // measured against says nothing more.
                    self.forget_rate();
                    self.base = Some(point);
                }
                classify(delta)
            }
            (Some((trust, centre, _)), Origin::Client | Origin::Ntp) => {
                let delta = signed(point.unix_us, trust.unix_us);
                if delta.unsigned_abs() > trust.error_us + point.error_us {
                    if fix.origin == Origin::Ntp {
                        self.disjoint = self.disjoint.saturating_add(1);
                        if self.disjoint >= DISJOINT_LIMIT {
                            self.trust = None;
                            self.centre = None;
                            self.forget_rate();
                            self.base = None;
                            self.disjoint = 0;
                        }
                    }
                    return Err(SyncError::Bounded);
                }
                let moved = narrow(trust, point);
                self.centre = Some(moved);
                if fix.origin == Origin::Ntp {
                    self.disjoint = 0;
                }
                classify(signed(moved.unix_us, centre.unix_us))
            }
        };
        self.syncs = self.syncs.saturating_add(1);
        self.last = Some((fix.origin, now));
        Ok(jump)
    }

    fn trusted(&mut self, point: Anchor) {
        self.trust = Some(point);
        self.centre = Some(point);
        self.disjoint = 0;
    }

    fn forget_rate(&mut self) {
        self.rate_ppb = 0;
        self.rate_bound_ppb = UNLEARNED_PPM * 1000;
        self.learned = false;
    }

    /// Measure the rate across the baseline, from one trusted fix to
    /// another, once it is long enough to do so within `LEARN_MAX_PPM`.
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
        let most = span.unsigned_abs() * u64::from(RATE_LIMIT_PPM.unsigned_abs()) / 1_000_000;
        if gained.unsigned_abs() > most {
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

    /// `anchor` carried to uptime `now`, its error grown by the rate
    /// bound; `None` past half the uptime clock's range.
    fn project(&self, anchor: Anchor, now: u32) -> Option<Anchor> {
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

    /// The trusted interval and the centre at `now`, and the error the
    /// reading reports, while the clock is valid.
    fn current(&self, now: u32) -> Option<(Anchor, Anchor, u64)> {
        let trust = self.project(self.trust?, now)?;
        let centre = self.project(self.centre?, now)?;
        let error_us = signed(centre.unix_us, trust.unix_us).unsigned_abs() + trust.error_us;
        (error_us.div_ceil(1000) <= MAX_ERROR_MS).then_some((trust, centre, error_us))
    }

    /// The time at uptime `now`, or `None` before the first fix and once
    /// the error passes `MAX_ERROR_MS`.
    pub fn read(&self, now: u32) -> Option<Reading> {
        let (_, centre, error_us) = self.current(now)?;
        let unix_ms = centre.unix_us / 1000;
        Some(Reading {
            unix_ms,
            offset_s: self.offset_at(unix_ms),
            error_ms: error_us.div_ceil(1000),
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

    /// The clock at uptime `now`, which the RTC reads as second `rtc_s`,
    /// as a record to keep across a warm reset; `None` while invalid, when
    /// the caller clears what it kept. The trusted interval and the reading
    /// are carried with the zone hint; the rate, the sync count and the
    /// baseline are not.
    pub fn carry(&self, now: u32, rtc_s: u32) -> Option<[u32; CARRY_WORDS]> {
        let (trust, centre, _) = self.current(now)?;
        Some(
            Carry {
                rtc_s,
                trust_us: trust.unix_us,
                trust_error_us: trust.error_us,
                centre_us: centre.unix_us,
                hint: self.hint,
            }
            .encode(),
        )
    }

    /// Resume a record [`carry`](Self::carry) made before a reset, at
    /// uptime `now`, which the RTC reads as `rtc_s` (`None`: unreadable).
    ///
    /// The RTC's count since the save is the time across the reset, within
    /// one second of resolution (both ends are whole seconds) and
    /// `RTC_PPM` of rate. The resumed interval is a trusted one, as the
    /// saved one was: it only ever grew from a trusted fix. It is no
    /// baseline for a rate, so the rate is learned again from the next two
    /// trusted fixes.
    pub fn resume(
        &mut self,
        now: u32,
        record: &[u32; CARRY_WORDS],
        rtc_s: Option<u32>,
        restart: Restart,
    ) -> Result<Resumed, NoResume> {
        if self.current(now).is_some() {
            return Err(NoResume::Set);
        }
        if restart == Restart::Cold {
            return Err(NoResume::Cold);
        }
        let carry = Carry::decode(record)?;
        let rtc_s = rtc_s.ok_or(NoResume::Rtc)?;
        let gap_s = rtc_s.checked_sub(carry.rtc_s).ok_or(NoResume::Backwards)?;
        if gap_s > CARRY_MAX_S {
            return Err(NoResume::Stale);
        }

        let gap_us = u64::from(gap_s) * 1_000_000;
        // ppm of a second is a microsecond.
        let grown_us = 1_000_000 + (u64::from(gap_s) + 1) * u64::from(RTC_PPM);
        let error_us = carry.trust_error_us + grown_us;
        let trust = Anchor {
            uptime_ms: now,
            unix_us: carry.trust_us + gap_us,
            error_us,
        };
        let centre = Anchor {
            uptime_ms: now,
            unix_us: carry.centre_us + gap_us,
            error_us,
        };
        // The record's times are inside 2026-2099 and the gap is minutes,
        // so the sums above cannot overflow.
        let reported_us = signed(centre.unix_us, trust.unix_us).unsigned_abs() + error_us;
        if reported_us.div_ceil(1000) > MAX_ERROR_MS {
            return Err(NoResume::Expired);
        }

        self.trusted(trust);
        self.centre = Some(centre);
        self.base = None;
        self.forget_rate();
        self.hint = carry.hint;
        Ok(Resumed {
            gap_s,
            error_ms: reported_us.div_ceil(1000),
        })
    }
}

/// A clock saved for a warm reset, as 16 little-endian words: the magic,
/// the version and flags, the RTC second, the trusted estimate and its
/// error and the reading (each in microseconds, low word first), the zone
/// hint, two reserved zero words, and a CRC-32 of the rest. The record
/// lives in RAM only, so its layout may change with any build: a build
/// that reads another's as `Corrupt` just starts unsynced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Carry {
    rtc_s: u32,
    trust_us: u64,
    trust_error_us: u64,
    centre_us: u64,
    hint: Option<Hint>,
}

/// `PLTC`.
const CARRY_MAGIC: u32 = 0x4354_4c50;
const CARRY_VERSION: u32 = 1;
const CARRY_HINT: u32 = 1 << 16;

impl Carry {
    fn encode(&self) -> [u32; CARRY_WORDS] {
        let split = |value: u64| {
            let [a, b, c, d, e, f, g, h] = value.to_le_bytes();
            [
                u32::from_le_bytes([a, b, c, d]),
                u32::from_le_bytes([e, f, g, h]),
            ]
        };
        let hint = self.hint.unwrap_or(Hint {
            offset_s: 0,
            next_change_s: 0,
            next_offset_s: 0,
        });
        let flags = if self.hint.is_some() { CARRY_HINT } else { 0 };

        let [trust_lo, trust_hi] = split(self.trust_us);
        let [error_lo, error_hi] = split(self.trust_error_us);
        let [centre_lo, centre_hi] = split(self.centre_us);
        let [change_lo, change_hi] = split(hint.next_change_s);
        let mut words = [
            CARRY_MAGIC,
            CARRY_VERSION | flags,
            self.rtc_s,
            trust_lo,
            trust_hi,
            error_lo,
            error_hi,
            centre_lo,
            centre_hi,
            hint.offset_s.cast_unsigned(),
            change_lo,
            change_hi,
            hint.next_offset_s.cast_unsigned(),
            0,
            0,
            0,
        ];
        words[CARRY_WORDS - 1] = carry_check(&words);
        words
    }

    fn decode(words: &[u32; CARRY_WORDS]) -> Result<Self, NoResume> {
        if words[0] != CARRY_MAGIC {
            return Err(NoResume::Empty);
        }
        let flags = words[1] & !0xffff;
        let valid = words[1] & 0xffff == CARRY_VERSION
            && flags & !CARRY_HINT == 0
            && words[13] == 0
            && words[14] == 0
            && words[CARRY_WORDS - 1] == carry_check(words);
        if !valid {
            return Err(NoResume::Corrupt);
        }

        let join = |lo: u32, hi: u32| (u64::from(hi) << 32) | u64::from(lo);
        let hint = Hint {
            offset_s: words[9].cast_signed(),
            next_change_s: join(words[10], words[11]),
            next_offset_s: words[12].cast_signed(),
        };
        let offsets = MIN_OFFSET_S..=MAX_OFFSET_S;
        let hint = if flags & CARRY_HINT == 0 {
            None
        } else if offsets.contains(&hint.offset_s) && offsets.contains(&hint.next_offset_s) {
            Some(hint)
        } else {
            return Err(NoResume::Corrupt);
        };
        let carry = Self {
            rtc_s: words[2],
            trust_us: join(words[3], words[4]),
            trust_error_us: join(words[5], words[6]),
            centre_us: join(words[7], words[8]),
            hint,
        };
        let times = MIN_UNIX_MS * 1000..MAX_UNIX_MS * 1000;
        if !times.contains(&carry.trust_us)
            || !times.contains(&carry.centre_us)
            || carry.trust_error_us > MAX_ERROR_MS * 1000
        {
            return Err(NoResume::Corrupt);
        }
        Ok(carry)
    }
}

/// CRC-32 of a record's words before the last.
fn carry_check(words: &[u32; CARRY_WORDS]) -> u32 {
    let mut bytes = [0; (CARRY_WORDS - 1) * 4];
    for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *chunk = word.to_le_bytes();
    }
    crate::store::slot::crc32(&bytes)
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
