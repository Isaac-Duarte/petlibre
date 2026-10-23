//! EXPERIMENTAL FW24 bring-up firmware for the PLAF103 on the PetLibre SDK:
//! the startup chime and requested tunes (`petlibre_hal::audio`), and the
//! signed network audio stream `petlibre-local speak` plays
//! (`petlibre_hal::stream`).
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-fw-speaker target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
