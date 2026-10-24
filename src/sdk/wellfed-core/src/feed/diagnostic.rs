//! Synthetic end-to-end pipeline check with a memory-only motor.
//! Never accesses board I/O.
//!
//! One single-portion feed runs through ADC scaling, sensor qualification,
//! the tick clock, the controller, the feedback queue and the motor gate.
//! The board app runs it at startup and the C ABI exports it, so a build
//! shows the core logic is linked and consistent; it says nothing about
//! hardware.

use crate::adc_scale::{AdcCalibration, SAMPLES};
use crate::clock::tick::TickClock;
use crate::feed::controller::{
    FeedController, FeedLimits, FeedRequest, FeedStartResult, FeedState, MotorCommand,
};
use crate::feed::feedback::{FeedbackDrive, FeedbackEvents, FeedbackFault};
use crate::feed::motor_gate::{MotorGate, MotorGateFault, MotorPins};
use crate::feed::progress::{BowlMode, Direction};
use crate::feed::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};

#[derive(Default)]
struct SimulatedMotor {
    a: bool,
    b: bool,
}

impl MotorPins for SimulatedMotor {
    fn write_pair(&mut self, a: bool, b: bool) -> bool {
        if !a && !b {
            return false;
        }
        self.a = a;
        self.b = b;
        true
    }
}

pub fn run() -> bool {
    run_pipeline().is_some()
}

/// `None` at the first step that fails.
fn run_pipeline() -> Option<()> {
    let calibration = AdcCalibration::decode(&[255; 4])?;
    // Compressed synthetic intervals, not settings for a physical board.
    let sensor_limits = SensorLimits {
        optical_settle_ms: 2,
        food_empty_delay_ms: 5,
        outlet_block_delay_ms: 5,
        power_settle_ms: 2,
        max_observation_gap_ms: 2,
        max_age_ms: 2,
        battery_min_scaled: 1650,
    };

    let feed_limits = FeedLimits {
        feedback_inhibit_ms: 5,
        feedback_timeout_ms: 30,
        direction_pause_ms: 3,
        reverse_ms: 3,
        max_duration_ms: 120,
        max_observation_gap_ms: 2,
        current_limit_ma: 300,
        max_portions: 1,
        max_reverse_attempts: 0,
    };

    let mut inputs = SensorInputs::new(&sensor_limits, &calibration)?;
    let mut controller = FeedController::new(&feed_limits, 0)?;
    let mut events = FeedbackEvents::new(8, 2, 0)?;
    let mut clock = TickClock::new(32_000, 0, 0)?;
    let mut gate = MotorGate::default();

    if !gate.init(SimulatedMotor::default(), 1, 3, 0) {
        return None;
    }

    let mut reading = SensorReading {
        gpio_valid: true,
        optics_enabled: true,
        outlet_high: true,
        dc_present: true,
        current_adc_valid: true,
        battery_adc_valid: true,
        current: [500; SAMPLES],
        battery: [2800; SAMPLES],
        ..SensorReading::default()
    };

    for tick in 0..=2 {
        let now = clock.read(tick * 32)?;
        inputs.update(now, Some(&reading));
    }

    let sample = inputs.sample(2, false, false);
    let request = FeedRequest {
        id: 1,
        portions: 1,
        mode: BowlMode::Single,
        direction: Direction::D0,
        ..FeedRequest::default()
    };

    if controller.start(&request, 2, &sample) != FeedStartResult::Accepted {
        return None;
    }

    for tick in 3..=7 {
        let now = clock.read(tick * 32)?;
        reading.outlet_high = now >= 5;
        inputs.update(now, Some(&reading));
        let sample = inputs.sample(now, false, false);
        if now == 7 && !events.push(now, true) {
            return None;
        }

        let drive = FeedbackDrive {
            healthy: gate.fault == MotorGateFault::Ok,
            applied_known: gate.applied_known,
            applied: gate.applied,
            applied_at: gate.applied_at,
        };

        if !events.process(&mut controller, now, &sample, drive)
            || !gate.request(controller.motor, now)
        {
            return None;
        }
    }

    (controller.state == FeedState::Complete
        && controller.progress.completed == 1
        && gate.applied_known
        && gate.applied == MotorCommand::Stop
        && gate.pins.as_ref().is_some_and(|motor| motor.a && motor.b)
        && events.count == 0
        && events.fault == FeedbackFault::Ok)
        .then_some(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn synthetic_pipeline_passes() {
        assert!(super::run());
    }
}
