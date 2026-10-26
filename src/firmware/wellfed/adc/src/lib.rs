//! EXPERIMENTAL bring-up firmware for the PLAF103 on the Wellfed SDK:
//! the read-only power-input probe (`wellfed_probes::adc`), sampling once a
//! second for as long as the build runs.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-adc target builds are KM4-only");

// The runtime's settings writer and Wi-Fi join run the feeder's record
// stores, which every PLAF103 firmware links.
use wellfed_feeder_runtime as _;

#[cfg(target_os = "none")]
mod app;
