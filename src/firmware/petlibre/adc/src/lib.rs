//! EXPERIMENTAL FW14 bring-up firmware for the PLAF103 on the PetLibre SDK:
//! the read-only power-input probe (`petlibre_probes::adc`), sampling once a
//! second for as long as the build runs.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-fw-adc target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
