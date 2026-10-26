//! The blinker: toggles the connection indicator and verifies each level
//! before holding it.
//!
//! A level is requested, left to settle, then read back. The connection
//! indicator is on Port A, which has no input readback, so a level is
//! accepted when latch, pad, directions and stopped outputs agree
//! (`leds::Decision::OutputOnly`). A refused request or a failed readback
//! drives every indicator LOW and ends the task; the blinker never retries.

use wellfed_sdk::Record;
use wellfed_sdk::hal::leds::{self, Decision, Led};

/// The indicator the blinker drives.
pub const LED: Led = Led::Connection;
/// How long each level is held, settle time included.
pub const HALF_PERIOD_MS: u32 = 500;
/// Levels reported after the first; later ones are silent unless one fails.
pub const REPORTED: u32 = 4;

/// One `PLBLK1` line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Record)]
#[record(tag = "PLBLK1")]
pub enum Record {
    /// A level was verified and is now held.
    #[record("level")]
    Level {
        #[record(with = Led::name)]
        led: Led,
        high: bool,
        #[record(with = decision_name)]
        decision: Decision,
        count: u32,
    },
    /// The blinker drove every indicator LOW and ended.
    #[record("stop")]
    Stop {
        #[record(with = Led::name)]
        led: Led,
        high: bool,
        reason: Stop,
    },
}

/// Why the blinker stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Record)]
pub enum Stop {
    /// The request was refused (outputs not stopped, or directions drifted).
    #[record("refused")]
    Refused,
    /// The settled readback did not accept the level.
    #[record("unverified")]
    Unverified {
        #[record(with = decision_name)]
        decision: Decision,
    },
}

fn decision_name(decision: Decision) -> &'static str {
    match decision {
        Decision::Verified => "verified",
        Decision::OutputOnly => "output-only",
        Decision::InputMismatch => "input-mismatch",
        Decision::Abort => "abort",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub record: Option<Record>,
    /// Milliseconds before the next step; zero ends the task.
    pub delay_ms: u32,
}

#[derive(Debug, Default)]
pub struct Blinker {
    high: bool,
    /// A level was requested and is settling before its readback.
    settling: bool,
    /// Levels verified so far.
    count: u32,
}

impl Blinker {
    pub fn step(&mut self) -> Step {
        let want = !self.high;
        if !self.settling {
            if !leds::request(LED, want) {
                return stop(want, Stop::Refused);
            }
            self.settling = true;
            return Step {
                record: None,
                delay_ms: leds::SETTLE_MS,
            };
        }

        self.settling = false;
        let decision = leds::observe(LED).decision(want);
        if !matches!(decision, Decision::Verified | Decision::OutputOnly) {
            return stop(want, Stop::Unverified { decision });
        }

        self.high = want;
        self.count = self.count.saturating_add(1);
        Step {
            record: (self.count <= REPORTED).then_some(Record::Level {
                led: LED,
                high: want,
                decision,
                count: self.count,
            }),
            delay_ms: HALF_PERIOD_MS - leds::SETTLE_MS,
        }
    }
}

/// Every indicator LOW, then end the task with why.
fn stop(high: bool, reason: Stop) -> Step {
    let _ = leds::all_low();
    Step {
        record: Some(Record::Stop {
            led: LED,
            high,
            reason,
        }),
        delay_ms: 0,
    }
}
