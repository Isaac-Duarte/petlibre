//! Exports declared by `sensor_inputs.h`.

use petlibre_core::adc_scale::AdcCalibration;
use petlibre_core::feed_controller::FeedSample;
use petlibre_core::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_sensor_inputs_init(
    inputs: *mut SensorInputs,
    limits: *const SensorLimits,
    calibration: *const AdcCalibration,
) -> bool {
    match (unsafe { limits.as_ref() }, unsafe { calibration.as_ref() }) {
        (Some(limits), Some(calibration)) => store(
            unsafe { inputs.as_mut() },
            SensorInputs::new(limits, calibration),
        ),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_sensor_inputs_update(
    inputs: *mut SensorInputs,
    now_ms: u32,
    reading: *const SensorReading,
) {
    unsafe { &mut *inputs }.update(now_ms, unsafe { reading.as_ref() });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_sensor_inputs_sample(
    inputs: *const SensorInputs,
    now_ms: u32,
    feedback_edge: bool,
    feedback_high: bool,
    sample: *mut FeedSample,
) {
    unsafe { *sample = (*inputs).sample(now_ms, feedback_edge, feedback_high) };
}
