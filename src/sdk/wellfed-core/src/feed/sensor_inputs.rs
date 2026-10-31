//! Food/outlet filtering, optical and power settling, and sample freshness.
//!
//! The owner feeds it one coherent acquisition per poll and composes a
//! [`FeedSample`] for the controller from it. A timing gap or an invalid
//! acquisition drops qualification back to unknown instead of keeping old
//! good data, and optical inputs count only after the emitters have settled.
//!
//! On the pack alone, power is healthy only while the pack is judged above
//! its floor ([`Floor`]): a sag under the floor shorter than its hold (a
//! motor's start-up surge) does not stop a feed, a longer one or one under
//! the cutoff does, and a pack once below must read the floor's clear level
//! before a feed starts again. An adapter change still unsettles power for
//! `power_settle_ms`, so a feed that sees one stops.

use crate::adc_scale::{AdcCalibration, SAMPLES};
use crate::feed::controller::FeedSample;
use crate::supply::{Floor, FloorLimits};
use crate::{HALF_RANGE, interval_valid};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorLimits {
    pub optical_settle_ms: u32,
    pub food_low_delay_ms: u32,
    pub outlet_block_delay_ms: u32,
    pub power_settle_ms: u32,
    pub max_observation_gap_ms: u32,
    pub max_age_ms: u32,
    /// The pack's floor, in the battery channel's scaled unit.
    pub battery: FloorLimits,
}

/// One coherent acquisition; no cached data relabeled with a new timestamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorReading {
    pub gpio_valid: bool,
    pub optics_enabled: bool,
    pub food_high: bool,
    pub outlet_high: bool,
    /// The feedback input's level, read in the same port access.
    pub feedback_high: bool,
    pub dc_present: bool,
    pub current_adc_valid: bool,
    pub battery_adc_valid: bool,
    pub current: [u16; SAMPLES],
    pub battery: [u16; SAMPLES],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorFilter {
    pub known: bool,
    pub good: bool,
    pub pending_bad: bool,
    pub pending_at: u32,
}

impl SensorFilter {
    fn update(&mut self, good: bool, now: u32, delay: u32) {
        if good {
            self.known = true;
            self.good = true;
            self.pending_bad = false;
        } else {
            if !self.pending_bad {
                self.pending_bad = true;
                self.pending_at = now;
            }
            if now.wrapping_sub(self.pending_at) >= delay {
                self.known = true;
                self.good = false;
            }
        }
    }
}

/// Serialized owner; fields are available for diagnostics, not caller mutation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorInputs {
    pub limits: SensorLimits,
    pub calibration: AdcCalibration,
    pub food: SensorFilter,
    pub outlet: SensorFilter,

    pub last_update_at: u32,
    pub optical_since: u32,
    pub power_since: u32,

    pub current_scaled: u32,
    pub battery_scaled: u32,
    /// The pack against its floor, judged on every acquisition, on the
    /// adapter too, so the verdict is current when it is pulled.
    pub floor: Floor,

    pub have_time: bool,
    pub timing_valid: bool,
    pub gpio_valid: bool,
    pub optics_on: bool,
    pub optical_ready: bool,
    pub power_known: bool,
    pub dc_present: bool,
    pub power_settled: bool,
    pub current_valid: bool,
    pub battery_valid: bool,
    /// The feedback input read LOW in the last valid acquisition.
    pub feedback_low: bool,
}

impl SensorInputs {
    pub fn new(limits: &SensorLimits, calibration: &AdcCalibration) -> Option<Self> {
        let valid = calibration.valid()
            && interval_valid(limits.optical_settle_ms)
            && interval_valid(limits.food_low_delay_ms)
            && interval_valid(limits.outlet_block_delay_ms)
            && interval_valid(limits.power_settle_ms)
            && interval_valid(limits.max_observation_gap_ms)
            && interval_valid(limits.max_age_ms)
            && limits.battery.valid();
        valid.then(|| Self {
            limits: *limits,
            calibration: *calibration,
            ..Self::default()
        })
    }

    fn reset_optics(&mut self) {
        self.optics_on = false;
        self.optical_ready = false;
        self.food = SensorFilter::default();
        self.outlet = SensorFilter::default();
    }

    fn reset_power(&mut self) {
        self.reset_optics();
        self.power_known = false;
        self.power_settled = false;
    }

    /// Invalid timing/acquisition resets qualification; it does not preserve good data.
    pub fn update(&mut self, now_ms: u32, reading: Option<&SensorReading>) {
        let gap = now_ms.wrapping_sub(self.last_update_at);
        self.timing_valid =
            !self.have_time || (gap < HALF_RANGE && gap <= self.limits.max_observation_gap_ms);
        self.have_time = true;
        self.last_update_at = now_ms;

        self.current_valid = false;
        self.battery_valid = false;
        self.gpio_valid = false;
        self.feedback_low = false;
        self.current_scaled = 0;
        self.battery_scaled = 0;

        let Some(reading) = reading.filter(|_| self.timing_valid) else {
            self.reset_power();
            return;
        };

        if reading.current_adc_valid
            && let Some(value) = self.calibration.scale_16(&reading.current)
        {
            self.current_scaled = value;
            self.current_valid = true;
        }
        if reading.battery_adc_valid
            && let Some(value) = self.calibration.scale_16(&reading.battery)
        {
            self.battery_scaled = value;
            self.battery_valid = true;
        }
        let battery = self.battery_valid.then_some(self.battery_scaled);
        self.floor.update(now_ms, battery, &self.limits.battery);

        self.gpio_valid = reading.gpio_valid;
        if !reading.gpio_valid {
            self.reset_power();
            return;
        }
        self.feedback_low = !reading.feedback_high;

        if !self.power_known || self.dc_present != reading.dc_present {
            self.power_known = true;
            self.dc_present = reading.dc_present;
            self.power_since = now_ms;
            self.power_settled = false;
        }
        if !self.power_settled
            && now_ms.wrapping_sub(self.power_since) >= self.limits.power_settle_ms
        {
            self.power_settled = true;
        }

        if !reading.optics_enabled {
            self.reset_optics();
            return;
        }
        if !self.optics_on {
            self.optics_on = true;
            self.optical_since = now_ms;
        }
        if !self.optical_ready
            && now_ms.wrapping_sub(self.optical_since) >= self.limits.optical_settle_ms
        {
            self.optical_ready = true;
        }

        if self.optical_ready {
            self.food
                .update(!reading.food_high, now_ms, self.limits.food_low_delay_ms);
            self.outlet.update(
                reading.outlet_high,
                now_ms,
                self.limits.outlet_block_delay_ms,
            );
        }
    }

    /// Whether the supply keeps a feed from starting: the feeder runs on
    /// its pack (the adapter read absent) and the pack is not judged above
    /// its floor, or its last acquisition failed.
    pub fn supply_low(&self) -> bool {
        self.power_known && !self.dc_present && !(self.battery_valid && self.floor.above())
    }

    /// Compose a controller snapshot. Feedback is supplied by the caller's
    /// ordered edge queue, once per edge; this module never caches or replays
    /// feedback. Unknown/disabled/settling optics and stale observations are
    /// not feed-ready.
    pub fn sample(&self, now_ms: u32, feedback_edge: bool, feedback_high: bool) -> FeedSample {
        let age = now_ms.wrapping_sub(self.last_update_at);
        let fresh = self.have_time
            && self.timing_valid
            && age < HALF_RANGE
            && age <= self.limits.max_age_ms;
        let gpio = fresh && self.gpio_valid;

        FeedSample {
            power_healthy: gpio
                && self.power_settled
                && (self.dc_present || (self.battery_valid && self.floor.above())),
            optical_valid: gpio && self.optical_ready && self.outlet.known,
            outlet_clear: self.outlet.good,
            current_valid: fresh && self.current_valid,
            current_ma: self.current_scaled,
            feedback_edge: gpio && feedback_edge,
            feedback_high,
            mark_low: gpio && self.feedback_low,
        }
    }

    /// The food beam's qualified reading: `Some(true)` with food in front of
    /// it. Known only while the emitters are on and settled.
    pub fn food_present(&self) -> Option<bool> {
        self.food.known.then_some(self.food.good)
    }
}
