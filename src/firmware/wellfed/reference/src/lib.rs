//! EXPERIMENTAL reference firmware for the PLAF103 research board, on the
//! Wellfed SDK.
//!
//! The firmware's tasks (`app`, target builds only) start inside the
//! `wellfed-sdk` runtime and run the bench probe its features select.
//! Host builds test the probes against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-reference target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
pub mod probes;
