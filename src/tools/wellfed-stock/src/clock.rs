//! Whether the server's `NTP` reply sets stock's clock (its
//! `calibrationTag`).
//!
//! Stock may hold the owner's feeding plans, and what it does with meals
//! that came due while its clock jumps forward is unverified: setting a
//! clock that is days behind could run them, with real food. Holding the
//! clock has its own cost: stock 1.1.15 refused an install with error 1214
//! while its clock stood two days behind (physical; that the clock is the
//! cause is likely, not confirmed statically). So the reply sets the clock
//! when stock's own reading is already within [`BOUND`] of ours, a step no
//! larger than its vendor cloud's sync would make, or when the owner asked
//! for it whatever it reads ([`Policy::Always`], `--calibrate-clock`).
//!
//! Stock dates each post with its own clock in `ts`, milliseconds since the
//! epoch, rounded to the second; its `NTP` request carries one too
//! (physical, UART captures of 1.1.15), so the request is its own evidence.
//! It asks once per MQTT connection.

use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use tracing::{info, warn};

/// How far stock's clock may stand from ours, either way, for the reply to
/// set it without the owner asking.
pub const BOUND: TimeDelta = TimeDelta::minutes(10);

/// A reading older than this may predate a restart of stock, which can
/// load an older time. Stock heartbeats about every 61 s, and its `NTP`
/// request dates itself, so a connected stock always has a fresher one.
const READING_AGE: TimeDelta = TimeDelta::seconds(90);

/// Readings outside 2020 to 2100 are an unset or corrupt clock, not a time.
const EARLIEST_MS: i64 = 1_577_836_800_000;
const LATEST_MS: i64 = 4_102_444_800_000;

/// What the owner allowed for a clock not known to be close.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// Set it only within [`BOUND`] (or with a captured empty plan table).
    #[default]
    Bounded,
    /// Set it whatever it reads: the owner accepts that stock may run meals
    /// that came due in the jump.
    Always,
}

/// Stock's clock as its latest dated message read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockClock {
    /// No message with a plausible `ts`, or none recent enough.
    Unknown,
    /// Its `ts`, and how far it stood ahead of ours (negative: behind).
    Reads { ts: i64, skew: TimeDelta },
}

/// Why the reply sets the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    /// A captured empty table: there are no plans to run.
    EmptyPlans,
    WithinBound,
    /// `--calibrate-clock`.
    Owner,
}

/// Why the reply leaves the clock alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// A captured nonempty table (a bench seeding config): stock runs
    /// plans this server sent it, and only the owner's flag sets its clock.
    CapturedPlans,
    Unknown,
    OutsideBound,
}

/// The reply's decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calibration {
    Sent(Sent),
    Held(Held),
}

impl Calibration {
    /// The reply's `calibrationTag`.
    pub fn tag(self) -> bool {
        matches!(self, Self::Sent(_))
    }
}

impl fmt::Display for Calibration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sent(Sent::EmptyPlans) => "sent: the captured plan table is empty",
            Self::Sent(Sent::WithinBound) => "sent: stock's clock is within 10 minutes of ours",
            Self::Sent(Sent::Owner) => "sent: --calibrate-clock",
            Self::Held(Held::CapturedPlans) => "held: the captured plan table has plans",
            Self::Held(Held::Unknown) => "held: stock's clock is unknown",
            Self::Held(Held::OutsideBound) => "held: stock's clock is over 10 minutes off",
        })
    }
}

/// Whether to set stock's clock, given the captured plan table (`None`
/// when nothing was captured, so stock may hold plans of its own).
///
/// A captured nonempty table keeps its earlier rule, held unless the owner
/// asks: those are bench seeding configs, whose stock is days behind and
/// runs plans written for the test.
pub fn decide(plans: Option<&[Value]>, clock: StockClock, policy: Policy) -> Calibration {
    match (plans, policy, clock) {
        (Some([]), _, _) => Calibration::Sent(Sent::EmptyPlans),
        (_, Policy::Always, _) => Calibration::Sent(Sent::Owner),
        (Some(_), Policy::Bounded, _) => Calibration::Held(Held::CapturedPlans),
        (None, Policy::Bounded, StockClock::Unknown) => Calibration::Held(Held::Unknown),
        (None, Policy::Bounded, StockClock::Reads { skew, .. }) if skew.abs() <= BOUND => {
            Calibration::Sent(Sent::WithinBound)
        }
        (None, Policy::Bounded, StockClock::Reads { .. }) => Calibration::Held(Held::OutsideBound),
    }
}

/// A dated message's reading.
#[derive(Clone, Copy, Debug)]
struct Reading {
    ts: i64,
    skew: TimeDelta,
    seen: DateTime<Utc>,
}

/// The owner's policy and stock's latest reading on the server's current
/// broker session.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    policy: Policy,
    latest: Option<Reading>,
}

impl Clock {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            latest: None,
        }
    }

    /// Note a stock message's `ts`, received `now`. Zero, absent or
    /// implausible values leave the last reading as it was.
    pub fn observe(&mut self, ts: Option<i64>, now: DateTime<Utc>) {
        let Some(ts) = ts.filter(|ts| (EARLIEST_MS..LATEST_MS).contains(ts)) else {
            return;
        };
        let skew = TimeDelta::milliseconds(ts - now.timestamp_millis());
        self.latest = Some(Reading {
            ts,
            skew,
            seen: now,
        });
    }

    /// The broker session ended: what stock read then says nothing of a
    /// later connection.
    pub fn forget(&mut self) {
        self.latest = None;
    }

    pub fn stock(&self, now: DateTime<Utc>) -> StockClock {
        match self.latest {
            Some(reading) if now - reading.seen <= READING_AGE => StockClock::Reads {
                ts: reading.ts,
                skew: reading.skew,
            },
            _ => StockClock::Unknown,
        }
    }

    /// The decision for an `NTP` request answered `now`, logged so a bench
    /// run records what stock's clock read.
    pub fn calibration(&self, plans: Option<&[Value]>, now: DateTime<Utc>) -> Calibration {
        let stock = self.stock(now);
        let calibration = decide(plans, stock, self.policy);

        let (ts, skew) = match stock {
            StockClock::Reads { ts, skew } => (ts.to_string(), format!("{} s", skew.num_seconds())),
            StockClock::Unknown => ("unknown".into(), "unknown".into()),
        };
        info!(stock_ts = %ts, skew = %skew, %calibration, "NTP request");
        if let Calibration::Held(_) = calibration {
            warn!(
                "stock's clock is left as it reads ({calibration}); stock may refuse an install (error 1214) until it is set: restart serve with --calibrate-clock to set it, accepting that stock may run meals that came due in the jump"
            );
        }
        calibration
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    fn reads(skew: TimeDelta) -> StockClock {
        StockClock::Reads { ts: 0, skew }
    }

    #[test]
    fn an_installer_sets_a_clock_within_ten_minutes_either_way() {
        for skew in [TimeDelta::zero(), BOUND, -BOUND, TimeDelta::seconds(-15)] {
            assert_eq!(
                decide(None, reads(skew), Policy::Bounded),
                Calibration::Sent(Sent::WithinBound)
            );
        }
    }

    #[test]
    fn an_installer_holds_a_clock_further_off_or_unknown() {
        let off = BOUND + TimeDelta::seconds(1);
        for skew in [off, -off, TimeDelta::days(-2)] {
            assert_eq!(
                decide(None, reads(skew), Policy::Bounded),
                Calibration::Held(Held::OutsideBound)
            );
        }
        assert_eq!(
            decide(None, StockClock::Unknown, Policy::Bounded),
            Calibration::Held(Held::Unknown)
        );
    }

    #[test]
    fn the_owner_s_flag_sets_any_clock() {
        let plans = [json!({"id": 1})];
        for (plans, clock) in [
            (None, StockClock::Unknown),
            (None, reads(TimeDelta::days(-2))),
            (Some(&plans[..]), reads(TimeDelta::days(-2))),
        ] {
            assert_eq!(
                decide(plans, clock, Policy::Always),
                Calibration::Sent(Sent::Owner)
            );
        }
    }

    #[test]
    fn a_captured_empty_table_always_sets_it() {
        for clock in [StockClock::Unknown, reads(TimeDelta::days(-2))] {
            assert_eq!(
                decide(Some(&[]), clock, Policy::Bounded),
                Calibration::Sent(Sent::EmptyPlans)
            );
        }
    }

    #[test]
    fn a_captured_nonempty_table_holds_it_even_within_the_bound() {
        let plans = [json!({"id": 1})];
        for clock in [StockClock::Unknown, reads(TimeDelta::zero())] {
            assert_eq!(
                decide(Some(&plans), clock, Policy::Bounded),
                Calibration::Held(Held::CapturedPlans)
            );
        }
    }

    #[test]
    fn the_latest_plausible_ts_is_the_reading_until_it_ages_or_the_session_ends() {
        let now = Utc.with_ymd_and_hms(2026, 10, 28, 21, 0, 0).unwrap();
        let mut clock = Clock::new(Policy::Bounded);
        assert_eq!(clock.stock(now), StockClock::Unknown);

        let behind = now.timestamp_millis() - 120_000;
        clock.observe(Some(behind), now);
        for ignored in [None, Some(0), Some(-5), Some(1_000), Some(i64::MAX)] {
            clock.observe(ignored, now);
        }
        let expected = StockClock::Reads {
            ts: behind,
            skew: TimeDelta::minutes(-2),
        };
        assert_eq!(clock.stock(now), expected);
        assert_eq!(clock.stock(now + READING_AGE), expected);
        assert_eq!(
            clock.stock(now + READING_AGE + TimeDelta::seconds(1)),
            StockClock::Unknown
        );

        let later = now + TimeDelta::seconds(30);
        let days_behind = (later - TimeDelta::days(2)).timestamp_millis();
        clock.observe(Some(days_behind), later);
        assert_eq!(
            clock.calibration(None, later),
            Calibration::Held(Held::OutsideBound)
        );

        clock.forget();
        assert_eq!(clock.stock(later), StockClock::Unknown);
    }
}
