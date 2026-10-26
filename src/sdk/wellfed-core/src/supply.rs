//! The supply: what a power sample says about the adapter and the battery
//! pack, with each kind of uncertainty named, and the pack's floor for
//! feeding.
//!
//! **Readings.** A board samples its pack through a divider and its adapter
//! on a detect pin. [`classify`] turns the latest sample into a [`Reading`]
//! that never passes an unknown off as a value: no sample at all is
//! [`Pack::Unread`], an old one [`Pack::Stale`] (and the adapter unknown
//! with it), and a value no pack of the board's can read is
//! [`Pack::Implausible`], kept for the record but never judged.
//!
//! **The floor.** [`Floor`] judges the pack against [`FloorLimits`] in the
//! unit the feeding owner samples (stock-scaled millivolts at the pin). A
//! pack below `floor` for `hold_ms` in a row, or below `cutoff` at all, is
//! below the floor; it is above again only once it reads `clear` or more.
//! The hold lets a motor's start-up surge sag the pack for a poll or two
//! without stopping the feed; the cutoff stops it at once when the sag is
//! deep enough to threaten the regulator; the hysteresis keeps a pack that
//! recovers at rest from feeding, sagging and stopping in a loop.

use crate::interval_valid;

/// What a board's pack readings are judged against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackLimits {
    /// Pack millivolts per thousand pin millivolts.
    pub divider_per_mille: u32,
    /// Below this at the pin no pack is fitted.
    pub absent_pin_mv: u32,
    /// A pack reading above this is implausible for the board's cells.
    pub max_pack_mv: u32,
    /// On the pack alone, a reading below this is implausible: the board
    /// would not be running to take it.
    pub min_running_mv: u32,
    /// A sample at least this old is stale.
    pub stale_ms: u32,
}

impl PackLimits {
    /// The pack's millivolts from the pin's, `None` when no pack is fitted.
    pub const fn pack_mv(&self, pin_mv: u32) -> Option<u32> {
        if pin_mv < self.absent_pin_mv {
            return None;
        }
        Some(pin_mv.saturating_mul(self.divider_per_mille) / 1000)
    }
}

/// The latest supply sample, however old.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub pin_mv: u32,
    pub adapter: bool,
    /// Milliseconds since it was taken.
    pub age_ms: u32,
}

/// The battery pack as a sample shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pack {
    /// No pack fitted: the pin reads near zero.
    Absent,
    /// The pack's millivolts.
    Reading { mv: u16 },
    /// A value outside what the board's pack can read, as read.
    Implausible { mv: u16 },
    /// Nothing sampled: the monitor has not run, its ADC failed, or the
    /// firmware does not sample the supply.
    Unread,
    /// The last sample is this old.
    Stale { age_ms: u32 },
}

/// A sample, judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// The adapter's detect pin; `None` with no fresh sample.
    pub adapter: Option<bool>,
    pub pack: Pack,
}

impl Reading {
    pub const UNREAD: Self = Self {
        adapter: None,
        pack: Pack::Unread,
    };

    /// The pack's millivolts when they are a plausible fresh reading.
    pub const fn pack_mv(&self) -> Option<u16> {
        match self.pack {
            Pack::Reading { mv } => Some(mv),
            Pack::Absent | Pack::Implausible { .. } | Pack::Unread | Pack::Stale { .. } => None,
        }
    }
}

/// Judge the latest `sample` against the board's `limits`.
pub fn classify(sample: Option<Sample>, limits: &PackLimits) -> Reading {
    let Some(sample) = sample else {
        return Reading::UNREAD;
    };
    if sample.age_ms >= limits.stale_ms {
        return Reading {
            adapter: None,
            pack: Pack::Stale {
                age_ms: sample.age_ms,
            },
        };
    }

    let pack = if let Some(mv) = limits.pack_mv(sample.pin_mv) {
        let mv16 = u16::try_from(mv).unwrap_or(u16::MAX);
        let too_low = !sample.adapter && mv < limits.min_running_mv;
        if mv > limits.max_pack_mv || too_low {
            Pack::Implausible { mv: mv16 }
        } else {
            Pack::Reading { mv: mv16 }
        }
    } else {
        Pack::Absent
    };
    Reading {
        adapter: Some(sample.adapter),
        pack,
    }
}

/// The pack's floor for feeding, in the feeding owner's sample unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FloorLimits {
    /// Below this for `hold_ms` in a row, the pack is below the floor.
    pub floor: u32,
    /// Once below, the pack is above again only at this or more.
    pub clear: u32,
    /// Below this, the pack is below the floor at once.
    pub cutoff: u32,
    pub hold_ms: u32,
}

impl FloorLimits {
    pub const fn valid(&self) -> bool {
        self.cutoff != 0
            && self.cutoff < self.floor
            && self.floor < self.clear
            && interval_valid(self.hold_ms)
    }
}

/// The pack against its floor. Unjudged until a reading at or above the
/// floor, a reading under the cutoff, or a hold under the floor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Floor {
    /// `None` until judged.
    below: Option<bool>,
    /// When the readings went under the floor (or missing), while they stay.
    under_since: Option<u32>,
}

impl Floor {
    /// One poll at `now_ms`: the pack's reading, or `None` when the
    /// acquisition failed, which counts as under the floor.
    pub fn update(&mut self, now_ms: u32, reading: Option<u32>, limits: &FloorLimits) {
        match reading {
            Some(value) if value >= limits.clear => {
                self.below = Some(false);
                self.under_since = None;
            }
            Some(value) if value >= limits.floor => {
                // Inside the band a judged pack keeps its verdict.
                self.below = Some(self.below.unwrap_or(false));
                self.under_since = None;
            }
            Some(value) if value < limits.cutoff => {
                self.below = Some(true);
                self.under_since.get_or_insert(now_ms);
            }
            Some(_) | None => {
                let since = *self.under_since.get_or_insert(now_ms);
                if now_ms.wrapping_sub(since) >= limits.hold_ms {
                    self.below = Some(true);
                }
            }
        }
    }

    /// Whether the pack is below its floor; `None` before it is judged.
    pub const fn below(&self) -> Option<bool> {
        self.below
    }

    /// Whether the pack may carry a feed: judged above the floor.
    pub const fn above(&self) -> bool {
        matches!(self.below, Some(false))
    }
}
