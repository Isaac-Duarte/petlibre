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
use wellfed_core::supply::FloorLimits;

/// Compressed durations exercise software; no deployment defaults are implied.
const LIMITS: SensorLimits = SensorLimits {
    optical_settle_ms: 2,
    food_empty_delay_ms: 5,
    outlet_block_delay_ms: 5,
    power_settle_ms: 2,
    max_observation_gap_ms: 2,
    max_age_ms: 2,
    battery: FloorLimits {
        floor: 1650,
        clear: 1710,
        cutoff: 1550,
        hold_ms: 4,
    },
};

/// Battery channel readings (raw) for scaled values around the floor,
/// with the default calibration.
const RAW_1536: u16 = 2100;
const RAW_1619: u16 = 2200;
const RAW_1700: u16 = 2297;
const RAW_1710: u16 = 2309;

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
    portion_ms: 120,
    max_observation_gap_ms: 2,
    mark_gap_ms: 1000,
    mark_low_ms: 1000,
    current_limit_ma: 300,
    current_max_ma: 1000,
    surge_ms: 0,
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

#[test]
fn the_mark_input_level_reaches_the_sample_only_from_a_valid_read() {
    let mut f = begin(0);
    assert!(f.snapshot().mark_low, "the fixture's PA18 reads LOW");

    f.raw.feedback_high = true;
    f.advance(1);
    assert!(!f.snapshot().mark_low);

    // A failed read shows no level: nothing to time a stuck input by.
    f.raw.feedback_high = false;
    f.raw.gpio_valid = false;
    f.advance(1);
    assert!(!f.snapshot().mark_low);
}

#[test]
fn the_food_reading_is_known_only_while_the_emitters_qualify_it() {
    // The fixture's food receiver reads LOW: food in front of the beam.
    let mut f = begin(0);
    assert_eq!(f.inputs.food_present(), None, "settling");
    f.advance(LIMITS.optical_settle_ms);
    assert_eq!(f.inputs.food_present(), Some(true));

    // Empty only after the filter's delay of steady HIGH readings.
    f.raw.food_high = true;
    f.advance(LIMITS.food_empty_delay_ms);
    assert_eq!(f.inputs.food_present(), Some(true));
    f.advance(1);
    assert_eq!(f.inputs.food_present(), Some(false));

    f.raw.optics_enabled = false;
    f.advance(1);
    assert_eq!(f.inputs.food_present(), None);
}

#[test]
fn raw_constants_scale_where_the_floor_tests_need_them() {
    let calibration = defaults();
    for (raw, scaled) in [
        (RAW_1536, 1536),
        (RAW_1619, 1619),
        (RAW_1700, 1700),
        (RAW_1710, 1710),
    ] {
        assert_eq!(calibration.scale_16(&[raw; SAMPLES]), Some(scaled));
    }
}

/// On the pack: a sag under the floor shorter than the hold leaves power
/// healthy (a motor's start-up surge); a longer one does not; and once
/// below, the pack must read the clear level, not just the floor, before
/// power is healthy again.
#[test]
fn a_sag_shorter_than_the_hold_is_tolerated_and_a_low_pack_needs_the_clear_level() {
    let mut f = begin(0);
    f.raw.dc_present = false;
    f.advance(3);
    assert!(f.snapshot().power_healthy && !f.inputs.supply_low());
    assert_eq!(f.inputs.floor.below(), Some(false));

    f.raw.battery = [RAW_1619; SAMPLES];
    f.advance(3);
    assert!(f.snapshot().power_healthy, "inside the hold");
    f.raw.battery = [2800; SAMPLES];
    f.advance(1);
    f.raw.battery = [RAW_1619; SAMPLES];
    f.advance(3);
    assert!(
        f.snapshot().power_healthy,
        "the hold restarts after a recovery"
    );

    f.advance(2);
    assert!(!f.snapshot().power_healthy && f.inputs.supply_low());
    assert_eq!(f.inputs.floor.below(), Some(true));

    f.raw.battery = [RAW_1700; SAMPLES];
    f.advance(5);
    assert!(
        !f.snapshot().power_healthy,
        "between the floor and the clear level"
    );
    f.raw.battery = [RAW_1710; SAMPLES];
    f.advance(1);
    assert!(f.snapshot().power_healthy && !f.inputs.supply_low());
}

#[test]
fn a_sag_under_the_cutoff_is_below_the_floor_at_once() {
    let mut f = begin(0);
    f.raw.dc_present = false;
    f.advance(3);
    assert!(f.snapshot().power_healthy);

    f.raw.battery = [RAW_1536; SAMPLES];
    f.advance(1);
    assert!(!f.snapshot().power_healthy && f.inputs.supply_low());
}

/// On the adapter the pack's floor does not matter, but it is still
/// judged, so pulling the adapter from a low pack finds it already low.
#[test]
fn the_floor_is_judged_on_the_adapter_and_only_matters_on_the_pack() {
    let mut f = begin(0);
    f.raw.battery = [RAW_1536; SAMPLES];
    f.advance(3);
    assert!(f.snapshot().power_healthy && !f.inputs.supply_low());
    assert_eq!(f.inputs.floor.below(), Some(true));

    f.raw.dc_present = false;
    f.advance(3);
    assert!(!f.snapshot().power_healthy && f.inputs.supply_low());
}

/// A failed battery conversion on the pack is not a reading: power is
/// unhealthy at once, as before, and the floor counts it under.
#[test]
fn a_failed_battery_conversion_on_the_pack_is_not_healthy() {
    let mut f = begin(0);
    f.raw.dc_present = false;
    f.advance(3);
    f.raw.battery_adc_valid = false;
    f.advance(1);
    assert!(!f.snapshot().power_healthy && f.inputs.supply_low());
    assert_eq!(f.inputs.floor.below(), Some(false));
    f.advance(4);
    assert_eq!(f.inputs.floor.below(), Some(true));
}
