//! EXPERIMENTAL AmebaD routines that both PLAF103 cores link.
//!
//! The KM0 image contains only this crate: the RAM flash park, the SDK flash
//! hooks, the park peer and the OTA trial step. `wellfed-hal` re-exports
//! these modules for the KM4 ports. Host builds run against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The KM4 provider fault callback calls the SDK's output guard.
pub use wellfed_amebad_sys as sdk;

pub mod cell;
pub mod flash;
pub mod trial;
