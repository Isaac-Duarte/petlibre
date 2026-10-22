//! EXPERIMENTAL KM4 first-boot image for the PLAF103 research board.
//!
//! The diagnostic application entry (`app`, target builds only) starts the
//! `petlibre-amebad` ports and runs the bench probe its features select.
//! Host builds test the probes against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-firstboot target builds are KM4-only");

#[cfg(target_os = "none")]
pub mod app;
pub mod probes;
