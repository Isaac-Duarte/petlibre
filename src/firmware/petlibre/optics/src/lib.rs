//! EXPERIMENTAL bring-up firmware for the PLAF103 on the PetLibre SDK:
//! the optics probe (`petlibre_probes::optics`), once per boot. It drives
//! the infrared emitters and reads the food, outlet and feedback inputs,
//! with the motor at STOP throughout.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-fw-optics target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
