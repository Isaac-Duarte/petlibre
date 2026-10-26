//! The supply in the loop: what the device API reports of it, what its
//! changes leave in the history, and the hold a brownout puts on feeding.
//!
//! **The reading.** Each pass judges the platform's latest sample
//! ([`wellfed_core::supply`]) with the feeding owner's verdict on the
//! pack's floor ([`Feeds::below_floor`]) into one [`Supply`]: the live
//! `Supply` metric and the events carry it as it is, unknowns named.
//!
//! **Events.** The adapter coming or going and the pack crossing its floor
//! are recorded as `Supply` events, at most one per [`EVENT_MS`]: a change
//! inside that window is recorded once it ends, as the supply then stands,
//! so an adapter that chatters or a pack that hovers at its floor costs one
//! event a minute, not one a change. The supply a boot first reads is its
//! baseline, not an event (the status and the metric report it); the first
//! change from it is recorded at once. A sample too old to trust (adapter
//! unknown) changes nothing here: staleness is reported, not recorded.
//!
//! **Brownout.** A boot whose reset was a brownout (the ROM's flag) holds
//! feeding as `Blocked::Battery` until the adapter is seen. The pack
//! recovers at rest, so its reading would let the next feed start, and the
//! drive could sag it into the same brownout again: a loop of resets that
//! each leave a feed uncertain, and that the storm breaker counts. A power
//! cycle (new cells) is a power-on reset, not a brownout, and clears it.

use wellfed_core::supply::{Pack, Reading};
use wellfed_proto::api::{Battery, ResetCause, Supply};

/// The least time between two `Supply` events.
pub const EVENT_MS: u32 = 60_000;

/// The wire's form of a judged reading, with the owner's verdict.
pub fn supply(reading: Reading, below_floor: Option<bool>) -> Supply {
    let battery = match reading.pack {
        Pack::Absent => Battery::Absent,
        Pack::Reading { mv } => Battery::Reading { mv },
        Pack::Implausible { mv } => Battery::Implausible { mv },
        Pack::Unread => Battery::Unread,
        Pack::Stale { age_ms } => Battery::Stale { age_ms },
    };
    Supply {
        adapter: reading.adapter,
        battery,
        below_floor,
    }
}

/// What a `Supply` event is recorded for: the adapter and the floor's
/// verdict, not the millivolts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Recorded {
    adapter: bool,
    below_floor: Option<bool>,
    /// `None` for the boot's baseline, which was not an event.
    at_ms: Option<u32>,
}

/// The supply's history and the brownout hold, kept by the loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// `None` until the first pass reads this boot's reset cause.
    brownout: Option<bool>,
    recorded: Option<Recorded>,
}

impl Watch {
    pub const fn new() -> Self {
        Self {
            brownout: None,
            recorded: None,
        }
    }

    /// Note the supply at `now_ms` on a boot that `reset` started, and
    /// lift the brownout hold once the adapter is seen. The supply to
    /// record as a `Supply` event, when a change is due.
    pub fn note(&mut self, now_ms: u32, reset: ResetCause, supply: Supply) -> Option<Supply> {
        let brownout = self.brownout.get_or_insert(reset == ResetCause::Brownout);
        if supply.adapter == Some(true) {
            *brownout = false;
        }

        let adapter = supply.adapter?;
        let Some(last) = self.recorded.as_mut() else {
            self.recorded = Some(Recorded {
                adapter,
                below_floor: supply.below_floor,
                at_ms: None,
            });
            return None;
        };
        // The owner's first verdict after the baseline, when it finds the
        // pack above its floor, is no change.
        if last.below_floor.is_none() && supply.below_floor == Some(false) {
            last.below_floor = Some(false);
        }
        let changed = last.adapter != adapter
            || (supply.below_floor.is_some() && supply.below_floor != last.below_floor);
        let due = last
            .at_ms
            .is_none_or(|at| now_ms.wrapping_sub(at) >= EVENT_MS);

        if !(changed && due) {
            return None;
        }
        self.recorded = Some(Recorded {
            adapter,
            below_floor: supply.below_floor,
            at_ms: Some(now_ms),
        });
        Some(supply)
    }

    /// Whether this boot followed a brownout and the adapter has not been
    /// seen since: feeding holds as `Blocked::Battery`.
    pub fn holds(&self) -> bool {
        self.brownout == Some(true)
    }
}
