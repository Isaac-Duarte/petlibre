//! EXPERIMENTAL bring-up firmware for the PLAF103 on the Wellfed SDK. Each
//! image variant brings up one area, with the motor at STOP throughout
//! unless the variant is `pulse`:
//!
//! - `optics`: the optics probe (`wellfed_probes::optics`), once per boot.
//!   It drives the infrared emitters and reads the food, outlet and feedback
//!   inputs.
//! - `buttons`: the read-only panel-button probe (`wellfed_probes::buttons`),
//!   logging every press, release and bounce, beside the SDK button service
//!   (`wellfed_sdk::buttons`), whose gestures its task reads.
//! - `adc`: the read-only power-input probe (`wellfed_probes::adc`), sampling
//!   once a second for as long as the build runs.
//! - `speaker`: the startup chime and the tune player (`wellfed_hal::audio`).
//! - `pulse`: the first supervised motor pulse (`wellfed_probes::pulse`),
//!   once per cold boot. Only this variant's motion policy marks an image
//!   that can drive the motor; motion builds stay a manual, attended step.
//! - `stall` and `fail`: trial images that must never confirm, so KM0 rolls
//!   each back. `stall` stops feeding the watchdog right after startup, as a
//!   hung candidate would, so KM0 counts the reset; `fail` fails the
//!   runtime's startup diagnostic on a determined error
//!   (`wellfed-sdk/trial-fail`), and this crate adds nothing to it.
//!
//! The normal image adds no task.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-bringup target builds are KM4-only");

// The runtime's settings writer and Wi-Fi join run the feeder's record
// stores, which every PLAF103 firmware links.
use wellfed_feeder_runtime as _;

#[cfg(target_os = "none")]
mod app;
