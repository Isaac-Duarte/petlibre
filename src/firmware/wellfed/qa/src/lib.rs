//! EXPERIMENTAL QA firmware for the PLAF103 on the Wellfed SDK: one guided
//! pass over every bring-up check, ending in a `PLQA1 result` line.
//!
//! The sequence (`sequence`) runs the optics and power probes unattended,
//! then prompts the operator through the buttons, indicators and speaker,
//! answering with the panel buttons. It never drives the motor. The
//! single-purpose firmwares beside it each run one probe for as long as a
//! bring-up session needs.
//!
//! Host builds test the sequence against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-qa target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
pub mod sequence;
