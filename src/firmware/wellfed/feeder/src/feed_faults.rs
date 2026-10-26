//! When the `feed-jam` and `sensor-fault` variants show a feed its fault,
//! and which. The SDK's `hal::feed_faults` shows it to the board's input
//! capture; everything after that is the product's own handling, so a
//! bench run with the hopper empty shows on hardware what a real jam or a
//! failed sensor would do: the reversals, the stop, the reason, and the
//! owner's hold.
//!
//! The requested portions choose the case, so the bench needs no new
//! device API operation:
//!
//! | Variant | Portions | Case | Armed |
//! | --- | --- | --- | --- |
//! | `feed-jam` | 1 | A jam the first reversal frees | 2 s into the drive |
//! | `feed-jam` | 2 | A jam no reversal frees | 2 s into the drive |
//! | `feed-jam` | 3 | The mark input stuck LOW | 2 s into the drive |
//! | `sensor-fault` | 1 | The marks lost | At the drive's start |
//! | `sensor-fault` | 2 | The outlet blocked | Before the feed starts |
//! | `sensor-fault` | 3 | The ADC lost | 2 s into the drive |
//!
//! Each feed is shown its fault once, and the fault is cleared as soon as
//! the feed has ended, so a later feed starts clean (an ADC the owner holds
//! lost stays so until a restart: the board latched it). The logic is
//! host-tested here; the variants' wiring is in `app`.

use crate::faults::INJECT_AFTER_MS;

/// Which variant runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Jam,
    Sensor,
}

/// What a feed is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    JamFrees,
    JamHolds,
    MarkStuckLow,
    MarksMissing,
    OutletBlocked,
    AdcLost,
}

/// When in a feed its case is armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// As soon as the feed is queued, before its emitters settle: the
    /// outlet must read blocked from its first qualified sample.
    Queued,
    /// When its drive starts.
    Drive,
    /// `INJECT_AFTER_MS` into its drive, past the start-up surge.
    IntoDrive,
}

impl Case {
    pub fn of(family: Family, portions: u16) -> Self {
        match (family, portions) {
            (Family::Jam, 0 | 1) => Self::JamFrees,
            (Family::Jam, 2) => Self::JamHolds,
            (Family::Jam, _) => Self::MarkStuckLow,
            (Family::Sensor, 0 | 1) => Self::MarksMissing,
            (Family::Sensor, 2) => Self::OutletBlocked,
            (Family::Sensor, _) => Self::AdcLost,
        }
    }

    pub const fn when(self) -> When {
        match self {
            Self::OutletBlocked => When::Queued,
            Self::MarksMissing => When::Drive,
            Self::JamFrees | Self::JamHolds | Self::MarkStuckLow | Self::AdcLost => When::IntoDrive,
        }
    }
}

/// The feed the owner has, from its queueing until its result: its id,
/// its requested portions, and whether its motor drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phase {
    pub feed: u64,
    pub portions: u16,
    pub driving: bool,
}

/// What the loop does to the board's capture now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Arm {
        feed: u64,
        case: Case,
    },
    /// The armed feed has ended.
    Clear {
        feed: u64,
    },
}

/// Watches the loop's passes for a feed to show its fault.
#[derive(Clone, Copy, Debug)]
pub struct Planner {
    family: Family,
    /// The feed whose fault is armed.
    armed: Option<u64>,
    /// The feed being driven and when the loop first saw it driving.
    seen: Option<(u64, u32)>,
    /// The last feed shown its fault; never twice.
    done: u64,
}

impl Planner {
    pub const fn new(family: Family) -> Self {
        Self {
            family,
            armed: None,
            seen: None,
            done: 0,
        }
    }

    /// One loop pass at `now_ms`, with the owner's feed, if any.
    pub fn step(&mut self, phase: Option<Phase>, now_ms: u32) -> Option<Action> {
        if let Some(feed) = self.armed
            && phase.is_none_or(|p| p.feed != feed)
        {
            self.armed = None;
            return Some(Action::Clear { feed });
        }

        let Some(phase) = phase.filter(|p| p.feed != self.done) else {
            self.seen = None;
            return None;
        };
        let case = Case::of(self.family, phase.portions);
        let due = match case.when() {
            When::Queued => true,
            When::Drive => phase.driving,
            When::IntoDrive => self.driven(phase, now_ms) >= Some(INJECT_AFTER_MS),
        };
        if !due {
            return None;
        }

        self.done = phase.feed;
        self.armed = Some(phase.feed);
        self.seen = None;
        Some(Action::Arm {
            feed: phase.feed,
            case,
        })
    }

    /// How long the loop has seen `phase`'s motor drive, from its first
    /// pass driving; `None` while it does not.
    fn driven(&mut self, phase: Phase, now_ms: u32) -> Option<u32> {
        if !phase.driving {
            self.seen = None;
            return None;
        }
        let since = match self.seen {
            Some((feed, at)) if feed == phase.feed => at,
            _ => {
                self.seen = Some((phase.feed, now_ms));
                now_ms
            }
        };
        Some(now_ms.wrapping_sub(since))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phase(feed: u64, portions: u16, driving: bool) -> Option<Phase> {
        Some(Phase {
            feed,
            portions,
            driving,
        })
    }

    #[test]
    fn portions_choose_the_case_and_when_it_is_armed() {
        let cases = [
            (Family::Jam, 1, Case::JamFrees, When::IntoDrive),
            (Family::Jam, 2, Case::JamHolds, When::IntoDrive),
            (Family::Jam, 3, Case::MarkStuckLow, When::IntoDrive),
            (Family::Sensor, 1, Case::MarksMissing, When::Drive),
            (Family::Sensor, 2, Case::OutletBlocked, When::Queued),
            (Family::Sensor, 3, Case::AdcLost, When::IntoDrive),
        ];
        for (family, portions, case, when) in cases {
            assert_eq!(Case::of(family, portions), case);
            assert_eq!(case.when(), when);
        }
    }

    #[test]
    fn a_jam_comes_two_seconds_into_the_drive_and_clears_with_the_feed() {
        let mut planner = Planner::new(Family::Jam);

        assert_eq!(planner.step(phase(4, 2, false), 0), None, "preparing");
        assert_eq!(planner.step(phase(4, 2, true), 100), None);
        assert_eq!(planner.step(phase(4, 2, true), 2099), None);
        assert_eq!(
            planner.step(phase(4, 2, true), 2100),
            Some(Action::Arm {
                feed: 4,
                case: Case::JamHolds
            })
        );
        assert_eq!(planner.step(phase(4, 2, true), 2200), None, "never twice");
        assert_eq!(planner.step(None, 9000), Some(Action::Clear { feed: 4 }));
        assert_eq!(planner.step(None, 9100), None);
    }

    #[test]
    fn a_blocked_outlet_is_armed_while_the_feed_is_queued() {
        let mut planner = Planner::new(Family::Sensor);

        assert_eq!(
            planner.step(phase(9, 2, false), 0),
            Some(Action::Arm {
                feed: 9,
                case: Case::OutletBlocked
            })
        );
        assert_eq!(planner.step(phase(9, 2, false), 2000), None);
        // The next feed takes the place of the last: cleared, then its own.
        assert_eq!(
            planner.step(phase(10, 1, true), 2100),
            Some(Action::Clear { feed: 9 })
        );
        assert_eq!(
            planner.step(phase(10, 1, true), 2120),
            Some(Action::Arm {
                feed: 10,
                case: Case::MarksMissing
            })
        );
    }

    #[test]
    fn a_feed_that_ends_before_its_fault_is_never_shown_it() {
        let mut planner = Planner::new(Family::Sensor);

        assert_eq!(planner.step(phase(3, 3, true), 0), None);
        assert_eq!(planner.step(None, 1000), None);
        // A drive seen again (a reversal's pause) counts afresh.
        assert_eq!(planner.step(phase(5, 3, true), 1100), None);
        assert_eq!(planner.step(phase(5, 3, false), 2000), None);
        assert_eq!(planner.step(phase(5, 3, true), 2500), None);
        assert_eq!(planner.step(phase(5, 3, true), 4499), None);
        assert!(planner.step(phase(5, 3, true), 4500).is_some());
    }
}
