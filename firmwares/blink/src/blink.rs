//! The blinker: toggles the connection indicator and verifies each level
//! before holding it.
//!
//! A level is requested, left to settle, then read back. The connection
//! indicator is on Port A, which has no input readback, so a level is
//! accepted when latch, pad, directions and stopped outputs agree
//! (`leds::Decision::OutputOnly`). A refused request or a failed readback
//! drives every indicator LOW and ends the task; the blinker never retries.

use core::fmt;

use wellfed_sdk::hal::leds::{self, Decision, Led};

/// The indicator the blinker drives.
pub const LED: Led = Led::Connection;
/// How long each level is held, settle time included.
pub const HALF_PERIOD_MS: u32 = 500;
/// Levels reported after the first; later ones are silent unless one fails.
pub const REPORTED: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// A level was verified and is now held.
    Level {
        high: bool,
        decision: Decision,
        count: u32,
    },
    /// The request was refused (outputs not stopped, or directions drifted).
    Refused { high: bool },
    /// The settled readback did not accept the level.
    Unverified { high: bool, decision: Decision },
}

/// One record as the body of a `PLBLK1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    let led = LED.name();
    match record {
        Record::Level {
            high,
            decision,
            count,
        } => write!(
            f,
            "level led={led} high={} decision={} count={count}",
            u8::from(high),
            decision_name(decision)
        ),
        Record::Refused { high } => {
            write!(f, "stop led={led} high={} reason=refused", u8::from(high))
        }
        Record::Unverified { high, decision } => write!(
            f,
            "stop led={led} high={} reason=unverified decision={}",
            u8::from(high),
            decision_name(decision)
        ),
    }
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
                return stop(Record::Refused { high: want });
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
            return stop(Record::Unverified {
                high: want,
                decision,
            });
        }

        self.high = want;
        self.count = self.count.saturating_add(1);
        Step {
            record: (self.count <= REPORTED).then_some(Record::Level {
                high: want,
                decision,
                count: self.count,
            }),
            delay_ms: HALF_PERIOD_MS - leds::SETTLE_MS,
        }
    }
}

/// Every indicator LOW, then end the task with `record`.
fn stop(record: Record) -> Step {
    let _ = leds::all_low();
    Step {
        record: Some(record),
        delay_ms: 0,
    }
}
