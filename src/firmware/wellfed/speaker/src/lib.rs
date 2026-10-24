//! EXPERIMENTAL bring-up firmware for the PLAF103 on the Wellfed SDK:
//! the startup chime and requested tunes (`wellfed_hal::audio`), and the
//! signed network audio stream `wellfed-local speak` plays
//! (`wellfed_hal::stream`).
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-speaker target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
