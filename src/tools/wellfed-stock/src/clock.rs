//! Whether the server's `NTP` reply sets stock's clock (its
//! `calibrationTag`).
//!
//! Stock 1.1.15 refused an install with error 1214 while its clock stood two
//! days behind (physical; that the clock is the cause is likely, not confirmed
//! statically), so the reply sets it. Stock reaches an installer's broker after
//! the Reset hold, which erased its plans, so no meal comes due in the jump.
//! A captured nonempty table is the one exception: the server has told stock to
//! run those plans, and what stock does with meals that came due while its
//! clock jumps forward is unverified, so the clock stays where it reads.
//!
//! Stock's `NTP` request carries its own clock in `ts`, milliseconds since the
//! epoch, rounded to the second (physical, UART captures of 1.1.15). It is
//! logged, so a bench run records what the clock read.

use std::fmt;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tracing::{info, warn};

/// The reply's decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calibration {
    Sent,
    /// A captured nonempty table: those plans may run in the jump.
    Held,
}

impl Calibration {
    /// The reply's `calibrationTag`.
    pub fn tag(self) -> bool {
        self == Self::Sent
    }
}

impl fmt::Display for Calibration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sent => "sent",
            Self::Held => "held: the captured plan table has plans",
        })
    }
}

/// Whether to set stock's clock, given the captured plan table (`None` when
/// nothing was captured).
pub fn decide(plans: Option<&[Value]>) -> Calibration {
    match plans {
        Some([_, ..]) => Calibration::Held,
        Some([]) | None => Calibration::Sent,
    }
}

/// The decision for an `NTP` request dated `ts` and answered `now`, logged
/// so a bench run records what stock's clock read.
pub fn calibration(plans: Option<&[Value]>, ts: Option<i64>, now: DateTime<Utc>) -> Calibration {
    let calibration = decide(plans);

    let skew = ts.map(|ts| (ts - now.timestamp_millis()) / 1000);
    info!(stock_ts = ?ts, skew_seconds = ?skew, %calibration, "NTP request");
    // 1214 is wellfed_client's `install::STOCK_CLOCK_UNSET`, the code the
    // installer turns into its own error; this crate does not depend on it.
    if calibration == Calibration::Held {
        warn!(
            "stock's clock is left as it reads ({calibration}); stock may refuse an install (error 1214) until it is set"
        );
    }
    calibration
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_clock_is_set_with_no_table_or_an_empty_one() {
        assert_eq!(decide(None), Calibration::Sent);
        assert_eq!(decide(Some(&[])), Calibration::Sent);
    }

    #[test]
    fn a_captured_nonempty_table_holds_it() {
        assert_eq!(decide(Some(&[json!({"id": 1})])), Calibration::Held);
    }
}
