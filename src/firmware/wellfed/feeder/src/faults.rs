//! When the fault-injection variants strike, and what with: the
//! `feed-stall` variant stalls the loop during a feed's drive, and the
//! `stop-fail` variant makes the stop of a feed's drive not take. Both
//! prove the supervision that does not depend on the loop or on the stop
//! path (the timer interrupt's motor lease and stop retry, the owner's beat,
//! the runtime's watchdog) on hardware, with real motion.
//!
//! Each feed is injected at most once, `INJECT_AFTER_MS` into its drive,
//! so the motor has started and the start-up surge has passed. The
//! requested portions choose how hard, so the bench picks the case with an
//! ordinary `feed --portions N` and no new device API operation:
//! - one portion: [`Severity::Short`], inside the owner's 1 s beat, so the
//!   timer interrupt ends the drive (the lease, or the stop's retry) and the
//!   owner records the feed durably; the owner has faulted, so its beat
//!   then lapses and the board resets with nothing pending;
//! - two or more: [`Severity::Long`], past the beat and the watchdog's
//!   longest expiry, so the board resets mid-feed and the next boot
//!   resolves the feed as uncertain.
//!
//! The logic is host-tested here; the variants' wiring is in `app`.

/// How far into a feed's drive the injection comes.
pub const INJECT_AFTER_MS: u32 = 2000;

/// The short stall: past the 200 ms motor lease, inside the 1 s beat the
/// owner promises while outputs may move.
pub const STALL_SHORT_MS: u32 = 600;
/// The long stall: past the 1 s beat plus the watchdog's longest expiry
/// (5.46 s at its 24 kHz bound), so the reset comes inside it. A loop that
/// comes back from it logs `stall end`, which says the watchdog did not
/// reset the board.
pub const STALL_LONG_MS: u32 = 8000;

/// The short failed stop: held past the motor lease, so the timer
/// interrupt retries the stop the cancel asked for about thirty times, and
/// inside the beat, so the retry that follows the lift ends the drive
/// before the watchdog could.
pub const HOLD_SHORT_MS: u32 = 300;
/// The long failed stop: held past the beat and the watchdog's longest
/// expiry, so only the watchdog's reset ends the drive. The hold then lifts
/// by itself, and the timer's next retry stops the motor, so the drive is
/// bounded even if the watchdog did not reset.
pub const HOLD_LONG_MS: u32 = 8000;

// The lengths sit where the supervision's bounds say they must: the short
// ones between the motor lease and the beat, the long ones past the beat
// plus the configured ceiling on the watchdog's expiry (above its 5.46 s
// bound), and every hold inside the SDK's 10 s cap (`hal::inject`, built
// only with the variant).
const _: () = {
    use wellfed_sdk::hal::supervision::config::WATCHDOG;

    let lease = crate::feeding::CONFIG.motor_lease_ms;
    let beat = crate::feeding::CONFIG.max_progress_gap_ms;
    assert!(lease < STALL_SHORT_MS && STALL_SHORT_MS < beat);
    assert!(lease < HOLD_SHORT_MS && HOLD_SHORT_MS < beat);
    assert!(STALL_LONG_MS > beat + WATCHDOG.max_expiry_ms);
    assert!(HOLD_LONG_MS > beat + WATCHDOG.max_expiry_ms);
    assert!(HOLD_LONG_MS <= 10_000);
};

/// The variant's fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The loop stops stepping the owner (`feed-stall`).
    Stall,
    /// The drive's stop does not take (`stop-fail`).
    FailedStop,
}

impl Fault {
    /// The code `PLINJ1` records print.
    pub const fn code(self) -> u8 {
        match self {
            Self::Stall => 1,
            Self::FailedStop => 2,
        }
    }
}

/// How hard to inject, from the feed's requested portions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Short,
    Long,
}

impl Severity {
    pub fn of(portions: u16) -> Self {
        if portions <= 1 {
            Self::Short
        } else {
            Self::Long
        }
    }

    /// The code `PLINJ1` records print.
    pub const fn code(self) -> u8 {
        match self {
            Self::Short => 1,
            Self::Long => 2,
        }
    }
}

/// What to do to a feed now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Injection {
    pub fault: Fault,
    pub feed: u64,
    pub severity: Severity,
}

impl Injection {
    /// How long the loop spins, or the STOP writes are held.
    pub const fn ms(&self) -> u32 {
        match (self.fault, self.severity) {
            (Fault::Stall, Severity::Short) => STALL_SHORT_MS,
            (Fault::Stall, Severity::Long) => STALL_LONG_MS,
            (Fault::FailedStop, Severity::Short) => HOLD_SHORT_MS,
            (Fault::FailedStop, Severity::Long) => HOLD_LONG_MS,
        }
    }
}

/// A feed whose motor the owner is driving: its id and requested portions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Driving {
    pub feed: u64,
    pub portions: u16,
}

/// Watches the loop's passes for a feed to inject into.
#[derive(Clone, Copy, Debug)]
pub struct Trigger {
    fault: Fault,
    /// The feed being driven and when the loop first saw it driving.
    seen: Option<(u64, u32)>,
    /// The last feed injected; never twice.
    done: u64,
}

impl Trigger {
    pub const fn new(fault: Fault) -> Self {
        Self {
            fault,
            seen: None,
            done: 0,
        }
    }

    /// One loop pass at `now_ms`, with the feed the owner drives, if any:
    /// the injection, once that feed has driven `INJECT_AFTER_MS`.
    pub fn step(&mut self, driving: Option<Driving>, now_ms: u32) -> Option<Injection> {
        let Some(driving) = driving.filter(|d| d.feed != self.done) else {
            self.seen = None;
            return None;
        };
        let since = match self.seen {
            Some((feed, at)) if feed == driving.feed => at,
            _ => {
                self.seen = Some((driving.feed, now_ms));
                now_ms
            }
        };
        if now_ms.wrapping_sub(since) < INJECT_AFTER_MS {
            return None;
        }

        self.done = driving.feed;
        self.seen = None;
        Some(Injection {
            fault: self.fault,
            feed: driving.feed,
            severity: Severity::of(driving.portions),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Driving = Driving {
        feed: 7,
        portions: 1,
    };

    #[test]
    fn a_feed_is_injected_once_two_seconds_into_its_drive() {
        let mut trigger = Trigger::new(Fault::Stall);

        assert_eq!(trigger.step(None, 0), None);
        assert_eq!(trigger.step(Some(ONE), 100), None);
        assert_eq!(trigger.step(Some(ONE), 2099), None);
        assert_eq!(
            trigger.step(Some(ONE), 2100),
            Some(Injection {
                fault: Fault::Stall,
                feed: 7,
                severity: Severity::Short
            })
        );

        assert_eq!(trigger.step(Some(ONE), 2120), None, "never twice");
        assert_eq!(trigger.step(Some(ONE), 9000), None);
        assert_eq!(trigger.step(None, 9100), None);
    }

    #[test]
    fn portions_choose_the_severity_and_a_new_feed_starts_its_own_clock() {
        let mut trigger = Trigger::new(Fault::FailedStop);
        let two = Driving {
            feed: 8,
            portions: 2,
        };

        assert_eq!(trigger.step(Some(ONE), 0), None);
        // The first feed ended before its injection; the next one counts
        // from its own drive.
        assert_eq!(trigger.step(Some(two), 1500), None);
        assert_eq!(trigger.step(Some(two), 3499), None);
        let injected = trigger.step(Some(two), 3500).unwrap();
        assert_eq!((injected.feed, injected.ms()), (8, HOLD_LONG_MS));

        assert_eq!(Severity::of(3), Severity::Long);
        let short = Injection {
            severity: Severity::of(1),
            ..injected
        };
        assert_eq!(short.ms(), HOLD_SHORT_MS);
        let stall = Injection {
            fault: Fault::Stall,
            ..injected
        };
        assert_eq!(stall.ms(), STALL_LONG_MS);
    }

    #[test]
    fn a_pause_in_the_drive_restarts_the_clock() {
        let mut trigger = Trigger::new(Fault::Stall);

        assert_eq!(trigger.step(Some(ONE), 0), None);
        assert_eq!(trigger.step(None, 1000), None);
        assert_eq!(trigger.step(Some(ONE), 1500), None);
        assert_eq!(trigger.step(Some(ONE), 3000), None);
        assert!(trigger.step(Some(ONE), 3500).is_some());
    }
}
