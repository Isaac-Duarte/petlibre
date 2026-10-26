//! EXPERIMENTAL AmebaD routines that both PLAF103 cores link.
//!
//! The KM0 image's archive is this crate: the RAM flash park, the SDK flash
//! hooks, the park peer, the OTA trial step and the panic handler.
//! `wellfed-hal` re-exports these modules for the KM4 ports, so every KM4
//! firmware links the same panic handler. Host builds run against the SDK
//! fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The KM4 provider fault callback calls the SDK's output guard.
pub use wellfed_amebad_sys as sdk;

pub mod cell;
pub mod flash;
#[cfg(target_os = "none")]
mod panic;
pub mod trial;
