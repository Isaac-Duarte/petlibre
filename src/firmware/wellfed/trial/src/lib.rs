//! EXPERIMENTAL test firmware for the PLAF103 on the Wellfed SDK:
//! trial images that must never confirm, so KM0 rolls each back.
//!
//! - `stall` stops feeding the watchdog right after startup, as a hung
//!   candidate would, so KM0 counts the reset.
//! - `fail` fails the runtime's startup diagnostic on a determined
//!   error (`wellfed-sdk/trial-fail`); this crate adds nothing to it.
//!
//! The normal image adds no task. Outputs stay stopped throughout.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-trial target builds are KM4-only");

// The runtime's settings writer and Wi-Fi join run the feeder's record
// stores, which every PLAF103 firmware links.
use wellfed_feeder_runtime as _;

#[cfg(target_os = "none")]
mod app;
