//! The LP ADC: the explicit clock and function enable, the efuse calibration
//! and bounded single conversions. The board's sensors and the runtime's
//! power monitor both use it, never at once: the monitor starts only once
//! the startup probe has released its board, and not at all while a
//! firmware has claimed the ADC (`wellfed_sdk::claim_adc`).

use wellfed_core::adc_scale::{AdcCalibration, MAX_RAW, SAMPLES};

use crate::sdk;

/// What `open` did, for diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opened {
    /// The LP ADC enable, with the registers before and after.
    pub clock: sdk::LpClock,
    /// Efuse calibration bytes 0x1d0..0x1d3, when all four read.
    pub efuse: Option<[u8; 4]>,
    /// Set only when the enable verified and the efuse decoded; the ADC is
    /// running only then.
    pub calibration: Option<AdcCalibration>,
}

/// Enable the LP ADC clock and function with readback, read the efuse
/// calibration and start the converter in manual channel select. Nothing
/// past the enable runs if a step fails.
pub fn open() -> Opened {
    let mut result = Opened {
        clock: sdk::adc_clock_enable(),
        ..Opened::default()
    };

    let mut efuse = [0u8; 4];
    for (byte, address) in efuse.iter_mut().zip(0x1d0u32..) {
        match sdk::efuse_read8(address) {
            Some(value) => *byte = value,
            None => return result,
        }
    }
    result.efuse = Some(efuse);

    let calibration = AdcCalibration::decode(&efuse);
    if !result.clock.verified() || calibration.is_none() {
        return result;
    }

    // PB1 and PB2, the external channels' pads.
    sdk::pad_disable(33);
    sdk::pad_disable(34);
    sdk::adc_init_manual();
    sdk::adc_cmd(true);
    result.calibration = calibration;
    result
}

/// `SAMPLES` conversions of `channel`, each polled at most `poll_limit`
/// times. False on the first conversion that never completes.
pub fn convert(channel: u32, poll_limit: u32, samples: &mut [u16; SAMPLES]) -> bool {
    for sample in samples.iter_mut() {
        let Some(raw) = convert_one(channel, poll_limit) else {
            return false;
        };
        *sample = raw;
    }
    true
}

/// One raw conversion of `channel`, or `None` if it never completes.
pub fn convert_one(channel: u32, poll_limit: u32) -> Option<u16> {
    sdk::adc_select(channel);
    sdk::adc_clear_fifo();
    sdk::adc_sw_trigger(true);
    let ready = (0..poll_limit).any(|_| sdk::adc_readable());
    sdk::adc_sw_trigger(false);
    if !ready {
        sdk::adc_clear_fifo();
        return None;
    }
    Some(sdk::adc_read() & MAX_RAW)
}

pub fn close() {
    sdk::adc_sw_trigger(false);
    sdk::adc_cmd(false);
}
