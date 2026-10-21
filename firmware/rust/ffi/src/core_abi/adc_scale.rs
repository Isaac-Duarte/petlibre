//! Exports declared by `adc_scale.h`.

use wellfed_core::adc_scale::{AdcCalibration, SAMPLES};

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_adc_calibration_valid(
    calibration: *const AdcCalibration,
) -> bool {
    unsafe { calibration.as_ref() }.is_some_and(AdcCalibration::valid)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_adc_calibration_decode(
    efuse: *const [u8; 4],
    calibration: *mut AdcCalibration,
) -> bool {
    match unsafe { efuse.as_ref() } {
        Some(efuse) if !calibration.is_null() => store(
            unsafe { calibration.as_mut() },
            AdcCalibration::decode(efuse),
        ),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_adc_scale_16(
    calibration: *const AdcCalibration,
    samples: *const [u16; SAMPLES],
    value: *mut u32,
) -> bool {
    match (unsafe { calibration.as_ref() }, unsafe { samples.as_ref() }) {
        (Some(calibration), Some(samples)) if !value.is_null() => {
            store(unsafe { value.as_mut() }, calibration.scale_16(samples))
        }
        _ => false,
    }
}
