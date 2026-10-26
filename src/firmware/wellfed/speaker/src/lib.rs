//! EXPERIMENTAL bring-up firmware for the PLAF103 on the Wellfed SDK:
//! the startup chime and the tune player (`wellfed_hal::audio`).
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-speaker target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
