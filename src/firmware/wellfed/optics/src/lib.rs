//! EXPERIMENTAL bring-up firmware for the PLAF103 on the Wellfed SDK:
//! the optics probe (`wellfed_probes::optics`), once per boot. It drives
//! the infrared emitters and reads the food, outlet and feedback inputs,
//! with the motor at STOP throughout.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-optics target builds are KM4-only");

// The runtime's settings writer and Wi-Fi join run the feeder's record
// stores, which every PLAF103 firmware links.
use wellfed_feeder_runtime as _;

#[cfg(target_os = "none")]
mod app;
