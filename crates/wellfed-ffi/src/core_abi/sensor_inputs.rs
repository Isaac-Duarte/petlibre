//! Exports declared by `sensor_inputs.h`.

use wellfed_core::adc_scale::AdcCalibration;
use wellfed_core::feed_controller::FeedSample;
use wellfed_core::sensor_inputs::{SensorInputs, SensorLimits, SensorReading};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_sensor_inputs_init(
    inputs: *mut SensorInputs,
    limits: *const SensorLimits,
    calibration: *const AdcCalibration,
) -> bool {
    // SAFETY: the C contract: `limits`, `calibration` are each null or valid for the call.
    match (unsafe { limits.as_ref() }, unsafe { calibration.as_ref() }) {
        (Some(limits), Some(calibration)) => store(
            // SAFETY: the C contract: `inputs` is null or valid for the call.
            unsafe { inputs.as_mut() },
            SensorInputs::new(limits, calibration),
        ),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_sensor_inputs_update(
    inputs: *mut SensorInputs,
    now_ms: u32,
    reading: *const SensorReading,
) {
    // SAFETY: the C contract: `inputs` must be valid: like the C original, this does not check for null; `reading` is null or valid for the call.
    unsafe { &mut *inputs }.update(now_ms, unsafe { reading.as_ref() });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_sensor_inputs_sample(
    inputs: *const SensorInputs,
    now_ms: u32,
    feedback_edge: bool,
    feedback_high: bool,
    sample: *mut FeedSample,
) {
    // SAFETY: the C contract: `sample` must be valid: like the C original, this does not check for null.
    unsafe { *sample = (*inputs).sample(now_ms, feedback_edge, feedback_high) };
}
