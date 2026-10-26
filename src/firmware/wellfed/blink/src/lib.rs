//! EXPERIMENTAL minimal firmware on the Wellfed SDK: one task blinks the
//! connection indicator once a second.
//!
//! It is the smallest firmware the runtime accepts, and the template for new
//! ones. Everything the feeder needs to stay safe and updatable (startup
//! checks, supervision, confirmation, settings, the update offer check and
//! the bank-0 recovery role) is the runtime's, so this crate only adds its
//! task. It never touches the motor or the infrared emitters: a request
//! refuses unless outputs read back stopped.
//!
//! Host builds test the blinker against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-blink target builds are KM4-only");

// The runtime's settings writer and Wi-Fi join run the feeder's record
// stores, which every PLAF103 firmware links.
use wellfed_feeder_runtime as _;

#[cfg(target_os = "none")]
mod app;
pub mod blink;
