//! EXPERIMENTAL FW16 bring-up firmware for the PLAF103 on the PetLibre SDK:
//! the first supervised motor pulse (`petlibre_probes::pulse`), once per
//! cold boot, in the `fw16-pulse` variant only. The normal image adds no
//! task, so no build without the variant's motion policy can drive the
//! motor. Motion builds stay a manual, attended step.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-fw-motor target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
