//! Bench probes selected by the diagnostic application. Each one reads back
//! stopped motor outputs; only the FW16 pulse probe, built with its own
//! feature, ever drives the motor.

pub mod adc;
pub mod flash;
pub mod optics;
pub mod pulse;
pub mod supervision;
pub mod wifi;

use petlibre_amebad::config::SYSTIMER_HZ;
use petlibre_amebad::sdk;

/// SYSTIMER ticks as milliseconds. At 32.768 kHz any u32 tick count fits.
fn ticks_ms(ticks: u32) -> u32 {
    u32::try_from(u64::from(ticks) * 1000 / u64::from(SYSTIMER_HZ)).unwrap_or(u32::MAX)
}

/// Milliseconds since a SYSTIMER sample, across one counter wrap.
fn elapsed_ms(since_raw: u32) -> u32 {
    ticks_ms(sdk::systimer_ticks().wrapping_sub(since_raw))
}
