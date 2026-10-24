//! EXPERIMENTAL bring-up firmware for the PLAF103 on the PetLibre SDK:
//! the read-only panel-button probe (`petlibre_probes::buttons`), logging
//! every press, release and bounce, beside the SDK button service
//! (`petlibre_sdk::buttons`), whose gestures its task reads.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-fw-buttons target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
