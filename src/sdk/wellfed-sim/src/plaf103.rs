//! The PLAF103 firmware's feeding limits, so the worker's owner meets the
//! measured mechanism ([`crate::plant`]) with the limits the feeder runs.
//!
//! The SDK cannot link the firmware that defines them (`wellfed-fw-feeder`'s
//! `feeding::CONFIG`), so they are copied here, and the firmware's own tests
//! hold the two equal: a change to either fails there.

use wellfed_core::feed::controller::FeedLimits;
use wellfed_core::feed::progress::BowlMode;
use wellfed_core::feed::sensor_inputs::SensorLimits;
use wellfed_hal::owner::OwnerConfig;
use wellfed_hal::sdk;
use wellfed_hal::supervision::config::{SYSTIMER_HZ, WATCHDOG};
use wellfed_hal::supervision::events::EventsConfig;
use wellfed_hal::supervision::supervise::LIVENESS_MS;

/// The vendor app's cap on one feed, which the journal pins.
pub const MAX_PORTIONS: u16 = 48;

pub const OWNER: OwnerConfig = OwnerConfig {
    feed: FeedLimits {
        feedback_inhibit_ms: 5000,
        feedback_timeout_ms: 15_000,
        direction_pause_ms: 500,
        reverse_ms: 1000,
        max_duration_ms: 12_000 * MAX_PORTIONS as u32,
        portion_ms: 12_000,
        max_observation_gap_ms: 100,
        mark_gap_ms: 11_000,
        mark_low_ms: 4000,
        current_limit_ma: 300,
        current_max_ma: 1000,
        surge_ms: 150,
        max_portions: MAX_PORTIONS,
        max_reverse_attempts: 3,
    },
    sensors: SensorLimits {
        optical_settle_ms: 100,
        food_empty_delay_ms: 1000,
        outlet_block_delay_ms: 1000,
        power_settle_ms: 100,
        max_observation_gap_ms: 100,
        max_age_ms: 100,
        battery_min_scaled: 1650,
    },
    events: EventsConfig {
        ticks_per_second: SYSTIMER_HZ,
        timer_period_ms: 10,
        max_feedback_age_ms: 100,
        feedback_capacity: 8,
    },
    max_progress_gap_ms: WATCHDOG.max_progress_gap_ms,
    idle_gap_ms: LIVENESS_MS,
    bowl_mode: BowlMode::Single,
    sample_period_ms: 20,
    prepare_timeout_ms: 2000,
    optics_max_on_ms: 12_000 * MAX_PORTIONS as u32 + 2000,
    motor_deadtime_ms: 50,
    motor_lease_ms: 200,
    adc_poll_limit: 10_000,
    wake_lock_id: sdk::PMU_USER_BASE,
    flash: core::ptr::null_mut(),
};

/// The longest one feed holds the outputs from rest: what the loop's claim
/// publishes, and so the longest a write or a join waits for them.
pub const FEED_BOUND_MS: u32 = OWNER.optics_max_on_ms + 2000;
