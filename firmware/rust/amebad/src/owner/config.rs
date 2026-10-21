//! Owner configuration and the static checks `Owner::start` applies.

use core::ptr;

use petlibre_core::adc_scale::AdcCalibration;
use petlibre_core::feed_controller::{FeedController, FeedLimits};
use petlibre_core::feed_progress::BowlMode;
use petlibre_core::feedback_events::CAPACITY;
use petlibre_core::flash_coord::{FlashCoord, FlashLimits};
use petlibre_core::interval_valid;
use petlibre_core::sensor_inputs::{SensorInputs, SensorLimits};

use crate::events::{self, EventsConfig};
use crate::sdk;
use crate::watchdog::WatchdogConfig;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OwnerConfig {
    pub feed: FeedLimits,
    pub sensors: SensorLimits,
    pub events: EventsConfig,
    pub watchdog: WatchdogConfig,
    pub bowl_mode: BowlMode,
    pub sample_period_ms: u32,
    pub prepare_timeout_ms: u32,
    pub optics_max_on_ms: u32,
    pub motor_deadtime_ms: u32,
    pub motor_lease_ms: u32,
    pub adc_poll_limit: u32,
    pub wake_lock_id: u32,
    /// Required external authority. Platform boot/transport establishes IDLE
    /// before start. Retain beyond owner STOPPED until global transport teardown.
    pub flash: *mut FlashCoord,
}

impl Default for OwnerConfig {
    fn default() -> Self {
        Self {
            feed: FeedLimits::default(),
            sensors: SensorLimits::default(),
            events: EventsConfig::default(),
            watchdog: WatchdogConfig::default(),
            bowl_mode: BowlMode::default(),
            sample_period_ms: 0,
            prepare_timeout_ms: 0,
            optics_max_on_ms: 0,
            motor_deadtime_ms: 0,
            motor_lease_ms: 0,
            adc_poll_limit: 0,
            wake_lock_id: 0,
            flash: ptr::null_mut(),
        }
    }
}

impl OwnerConfig {
    pub fn valid(&self) -> bool {
        let c = self;
        if !c.watchdog.valid()
            || c.watchdog.systimer_hz != c.events.ticks_per_second
            || c.watchdog.max_progress_gap_ms <= c.sample_period_ms
            || !interval_valid(c.sample_period_ms)
            || !interval_valid(c.prepare_timeout_ms)
            || !interval_valid(c.optics_max_on_ms)
            || c.motor_deadtime_ms == 0
            || !interval_valid(c.motor_lease_ms)
            || c.motor_lease_ms <= c.motor_deadtime_ms
            || c.sample_period_ms >= c.motor_lease_ms / 3
            || c.sample_period_ms > c.sensors.max_age_ms
            || c.sample_period_ms > c.sensors.max_observation_gap_ms
            || c.sample_period_ms > c.feed.max_observation_gap_ms
            || c.sample_period_ms > c.events.max_feedback_age_ms
            || c.events.max_feedback_age_ms >= c.feed.feedback_inhibit_ms
            || c.sensors.optical_settle_ms >= c.prepare_timeout_ms
            || c.sensors.power_settle_ms >= c.prepare_timeout_ms
            || u64::from(c.prepare_timeout_ms) + u64::from(c.feed.max_duration_ms)
                > u64::from(c.optics_max_on_ms)
            || c.events.ticks_per_second < 1000
            || c.events.ticks_per_second > 1_000_000
            || c.events.timer_period_ms == 0
            || c.events.timer_period_ms >= c.motor_lease_ms / 3
            || c.events.feedback_capacity == 0
            || usize::from(c.events.feedback_capacity) > CAPACITY
            || c.adc_poll_limit == 0
            || c.adc_poll_limit > 1_000_000
            || c.wake_lock_id < sdk::PMU_USER_BASE
            || c.wake_lock_id > sdk::PMU_MAX
            || c.wake_lock_id == 17
            || !c.bowl_mode.valid()
        {
            return false;
        }
        // Only validate structure/ranges here; real efuse calibration is read by open.
        let calibration = AdcCalibration {
            offset: 2480,
            gain: 12050,
            ..AdcCalibration::default()
        };
        events::period_ticks(&c.events).is_some()
            && FeedController::new(&c.feed, 0).is_some()
            && SensorInputs::new(&c.sensors, &calibration).is_some()
    }

    /// Whether a whole flash lease fits inside watchdog supervision. During a
    /// lease the owner cannot checkpoint, so the gap between two checkpoints
    /// spans up to one sample period before the lease, the entire lease
    /// budget, one sample period after it, and two timer periods of
    /// scheduling/timer granularity before the first post-lease checkpoint
    /// (it also needs a fresh timer interrupt). That sum must stay below
    /// `max_progress_gap_ms`. The hardware counter, whose shortest expiry
    /// `WatchdogConfig::valid` already requires to exceed twice the progress
    /// gap, then also covers an IRQ-masked lease plus the rate-limited refresh
    /// offset (`min_refresh_ms`, below the gap).
    pub fn lease_fits(&self, limits: &FlashLimits) -> bool {
        let needed = u64::from(limits.total_ms)
            + 2 * u64::from(self.sample_period_ms)
            + 2 * u64::from(self.events.timer_period_ms);
        limits.total_ms != 0 && needed < u64::from(self.watchdog.max_progress_gap_ms)
    }
}
