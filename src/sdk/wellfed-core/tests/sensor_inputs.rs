//! Tests for sensor normalization: ADC scaling and calibration, optical and
//! power settling, outlet-blockage and food filtering, sample freshness and
//! clock wrap, and the handoff into the feeding controller.
//!
//! An input reads as invalid until it has settled, and a gap, backward time
//! or bad ADC sample discards its qualification, so an optical or power
//! change stops an active feed.

use wellfed_core::adc_scale::{AdcCalibration, SAMPLES};
use wellfed_core::feed::controller::{
    FeedController, FeedLimits, FeedReason, FeedRequest, FeedSample, FeedStartResult, FeedState,
    MotorCommand,
};
use wellfed_core::feed::progress::{BowlMode, Direction};
use wellfed_core::feed::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};

/// Compressed durations exercise software; no deployment defaults are implied.
const LIMITS: SensorLimits = SensorLimits {
    optical_settle_ms: 2,
    food_empty_delay_ms: 5,
    outlet_block_delay_ms: 5,
    power_settle_ms: 2,
    max_observation_gap_ms: 2,
    max_age_ms: 2,
    battery_min_scaled: 1650,
};

struct Fixture {
    inputs: SensorInputs,
    raw: SensorReading,
    now: u32,
}

fn defaults() -> AdcCalibration {
    AdcCalibration::decode(&[255; 4]).unwrap()
}

fn begin_with(settings: &SensorLimits, now: u32) -> Fixture {
    let raw = SensorReading {
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

    let mut f = Fixture {
        inputs: SensorInputs::new(settings, &defaults()).unwrap(),
        raw,
        now,
    };

    f.inputs.update(now, Some(&f.raw));
    f
}

fn begin(now: u32) -> Fixture {
    begin_with(&LIMITS, now)
}

impl Fixture {
    fn advance(&mut self, elapsed: u32) {
        for _ in 0..elapsed {
            self.now = self.now.wrapping_add(1);
            self.inputs.update(self.now, Some(&self.raw));
        }
    }

    fn snapshot(&self) -> FeedSample {
        self.inputs.sample(self.now, false, false)
    }
}

#[test]
fn adc_defaults_and_threshold() {
    let calibration = defaults();
    assert_eq!((calibration.offset, calibration.gain), (2480, 12050));
    assert!(calibration.default_offset && calibration.default_gain);

    let mut samples = [610; SAMPLES];
    assert_eq!(calibration.scale_16(&samples), Some(300));
    samples[15] = 611; // Floor the mean before applying the gain.
    assert_eq!(calibration.scale_16(&samples), Some(300));
    assert_eq!(calibration.scale_16(&[611; SAMPLES]), Some(301));

    let calibration = AdcCalibration::decode(&[255, 255, 0xe8, 3]).unwrap();
    assert_eq!((calibration.offset, calibration.gain), (2480, 1000));
    assert!(calibration.default_offset && !calibration.default_gain);

    let calibration = AdcCalibration::decode(&[0, 0, 0, 128]).unwrap();
    assert_eq!(calibration.scale_16(&[4095; SAMPLES]), Some(1249));
}

#[test]
fn adc_saturation_and_numeric_bounds() {
    let calibration = defaults();
    assert_eq!(calibration.scale_16(&[0; SAMPLES]), Some(0));
    assert_eq!(calibration.scale_16(&[248; SAMPLES]), Some(0));
    assert_eq!(calibration.scale_16(&[4095; SAMPLES]), Some(3192));

    let calibration = AdcCalibration {
        offset: 0,
        gain: 1,
        ..AdcCalibration::default()
    };
    assert_eq!(calibration.scale_16(&[4095; SAMPLES]), Some(40_950_000));
}

#[test]
fn invalid_adc_never_reuses_a_value() {
    let mut calibration = defaults();
    let mut samples = [500; SAMPLES];
    samples[0] = 4096;
    assert_eq!(calibration.scale_16(&samples), None);

    assert!(AdcCalibration::decode(&[0; 4]).is_none());
    calibration.gain = 0;
    assert_eq!(calibration.scale_16(&samples), None);
}

#[test]
fn optics_are_unknown_until_settled() {
    let mut f = begin(0);
    assert!(!f.snapshot().optical_valid);

    f.advance(2);
    assert!(f.snapshot().optical_valid && f.snapshot().outlet_clear);

    f.raw.optics_enabled = false;
    f.advance(1);
    assert!(!f.snapshot().optical_valid && !f.inputs.food.known);

    f.raw.optics_enabled = true;
    f.advance(1);
    assert!(!f.snapshot().optical_valid);
    f.advance(2);
    assert!(f.snapshot().optical_valid);
}

#[test]
fn passage_pulses_and_sustained_blockage() {
    let mut f = begin(0);
    f.advance(2);

    f.raw.outlet_high = false;
    f.advance(4);
    assert!(f.snapshot().outlet_clear);

    f.raw.outlet_high = true;
    f.advance(1);
    f.raw.outlet_high = false;
    f.advance(5);
    assert!(f.snapshot().outlet_clear);
    f.advance(1);
    assert!(f.snapshot().optical_valid && !f.snapshot().outlet_clear);

    f.raw.outlet_high = true;
    f.advance(1);
    assert!(f.snapshot().outlet_clear);

    f.raw.food_high = true;
    f.advance(5);
    assert!(f.inputs.food.good);
    f.advance(1);
    assert!(f.inputs.food.known && !f.inputs.food.good);

    f.raw.food_high = false;
    f.advance(1);
    assert!(f.inputs.food.good);
}

#[test]
fn stale_gap_and_backward_time_discard_qualification() {
    let mut f = begin(0);
    f.advance(2);
    let sample = f.inputs.sample(5, true, true);
    assert!(
        !sample.current_valid
            && !sample.optical_valid
            && !sample.power_healthy
            && !sample.feedback_edge
    );

    f.now = 5;
    f.inputs.update(f.now, Some(&f.raw));
    assert!(!f.snapshot().current_valid && !f.snapshot().optical_valid);
    f.advance(3);
    assert!(f.snapshot().optical_valid && f.snapshot().power_healthy);

    f.inputs.update(7, Some(&f.raw));
    assert!(!f.inputs.timing_valid && !f.inputs.outlet.known);
}

#[test]
fn power_transition_and_battery_failure() {
    let mut f = begin(0);
    f.advance(2);
    assert!(f.snapshot().power_healthy);

    f.raw.dc_present = false;
    f.advance(1);
    assert!(!f.snapshot().power_healthy);
    f.advance(2);
    assert!(f.snapshot().power_healthy);

    f.raw.battery = [1500; SAMPLES];
    f.advance(1);
    assert!(!f.snapshot().power_healthy);

    f.raw.dc_present = true;
    f.raw.battery_adc_valid = false;
    f.advance(3);
    assert!(f.snapshot().power_healthy && !f.inputs.battery_valid);

    f.raw.current[0] = 4096;
    f.advance(1);
    assert!(!f.snapshot().current_valid);
}

#[test]
fn initially_blocked_outlet_is_not_assumed_clear() {
    let mut f = begin(0);
    f.raw.outlet_high = false;
    f.advance(6);
    assert!(!f.snapshot().optical_valid);
    f.advance(1);
    assert!(f.snapshot().optical_valid && !f.snapshot().outlet_clear);
}

#[test]
fn clock_wrap_does_not_unsettle_stable_inputs() {
    let mut f = begin(u32::MAX - 1);
    f.advance(2);
    assert!(f.now == 0 && f.snapshot().optical_valid && f.snapshot().power_healthy);

    let settings = SensorLimits {
        max_observation_gap_ms: 0x7fff_ffff,
        ..LIMITS
    };
    let mut f = begin_with(&settings, 0);
    f.advance(2);

    // Two valid large intervals wrap uptime after qualification, without replay.
    f.now = 0x8000_0001;
    f.inputs.update(f.now, Some(&f.raw));
    f.now = 0;
    f.inputs.update(f.now, Some(&f.raw));
    assert!(f.snapshot().optical_valid && f.snapshot().power_healthy);
}

#[test]
fn invalid_sensor_configuration() {
    let mut calibration = defaults();
    assert!(
        SensorInputs::new(
            &SensorLimits {
                optical_settle_ms: 0,
                ..LIMITS
            },
            &calibration
        )
        .is_none()
    );
    assert!(
        SensorInputs::new(
            &SensorLimits {
                max_age_ms: 0x8000_0000,
                ..LIMITS
            },
            &calibration
        )
        .is_none()
    );

    calibration.gain = 0;
    assert!(SensorInputs::new(&LIMITS, &calibration).is_none());
}

const FEED_LIMITS: FeedLimits = FeedLimits {
    feedback_inhibit_ms: 5,
    feedback_timeout_ms: 30,
    direction_pause_ms: 3,
    reverse_ms: 3,
    max_duration_ms: 120,
    max_observation_gap_ms: 2,
    current_limit_ma: 300,
    max_portions: 5,
    max_reverse_attempts: 1,
};

fn start_feed(f: &Fixture) -> FeedController {
    let mut controller = FeedController::new(&FEED_LIMITS, 0).unwrap();
    let request = FeedRequest {
        id: 1,
        portions: 1,
        mode: BowlMode::Single,
        direction: Direction::D0,
        ..FeedRequest::default()
    };
    assert_eq!(
        controller.start(&request, f.now, &f.snapshot()),
        FeedStartResult::Accepted
    );
    controller
}

#[test]
fn normalized_passage_can_complete_a_feed() {
    let mut f = begin(0);
    f.advance(2);
    let mut controller = start_feed(&f);

    for i in 0..5 {
        f.raw.outlet_high = i >= 3;
        f.advance(1);
        let sample = f.inputs.sample(f.now, i == 4, true);
        assert!(sample.outlet_clear);
        controller.update(f.now, Some(&sample));
    }

    assert!(controller.state == FeedState::Complete && controller.progress.completed == 1);
    assert_eq!(controller.motor, MotorCommand::Stop);
}

#[test]
fn optical_loss_and_power_change_stop_an_active_feed() {
    for fault in 0..2 {
        let mut f = begin(0);
        f.advance(2);
        let mut controller = start_feed(&f);
        if fault == 0 {
            f.raw.optics_enabled = false;
        } else {
            f.raw.dc_present = false;
        }

        f.advance(1);
        controller.update(f.now, Some(&f.snapshot()));
        assert!(controller.state == FeedState::Fault && controller.motor == MotorCommand::Stop);
        assert_eq!(
            controller.reason,
            if fault == 0 {
                FeedReason::Sensor
            } else {
                FeedReason::Power
            }
        );
    }
}
