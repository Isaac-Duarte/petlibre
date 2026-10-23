//! EXPERIMENTAL FW11 test firmware for the PLAF103 on the PetLibre SDK:
//! trial images that must never confirm, so KM0 rolls each back.
//!
//! - `fw11-reset` stops feeding the watchdog right after startup, as a hung
//!   candidate would, so KM0 counts the reset.
//! - `fw11-fail` fails the runtime's startup diagnostic on a determined
//!   error (`petlibre-sdk/fw11-fail`); this crate adds nothing to it.
//!
//! The normal image adds no task. Outputs stay stopped throughout.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-fw-trial target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
