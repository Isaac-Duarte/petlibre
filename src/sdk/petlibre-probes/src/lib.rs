//! EXPERIMENTAL bench probes for the PLAF103, shared by the firmwares that
//! run them: each single-purpose bring-up firmware runs one, and the QA
//! firmware runs several in sequence.
//!
//! A probe is a step function over its own state; the firmware owns the task
//! that steps it (the build's stack audit finds tasks only in the firmware's
//! own sources). Each probe reads back stopped motor outputs; only the motor
//! pulse probe ever drives the motor, and only the `motor` firmware runs it.
//! The flash and supervision probes are retained for their host tests; no
//! firmware links them, and the startup audit refuses an image that does.
//!
//! Host builds test the probes against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]

pub mod adc;
pub mod buttons;
pub mod flash;
pub mod optics;
pub mod pulse;
pub mod speaker;
pub mod supervision;

use core::fmt;

use petlibre_hal::supervision::config::SYSTIMER_HZ;
use petlibre_hal::{sdk, startup};

/// Logs one record line, `PLxxx1 …\r\n`. A line longer than the report
/// buffer is dropped rather than truncated, so a parser never sees half a
/// record.
pub fn log(line: fmt::Arguments<'_>) {
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(line) {
        sdk::log(line);
    }
}

/// SYSTIMER ticks as milliseconds. At 32.768 kHz any u32 tick count fits.
fn ticks_ms(ticks: u32) -> u32 {
    u32::try_from(u64::from(ticks) * 1000 / u64::from(SYSTIMER_HZ)).unwrap_or(u32::MAX)
}

/// Milliseconds since a SYSTIMER sample, across one counter wrap.
fn elapsed_ms(since_raw: u32) -> u32 {
    ticks_ms(sdk::systimer_ticks().wrapping_sub(since_raw))
}
