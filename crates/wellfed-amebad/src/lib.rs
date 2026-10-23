//! EXPERIMENTAL AmebaD ports for the PLAF103 research board.
//!
//! Host builds run against [`sdk::fake`]; KM4 target builds need the `sdk`
//! feature and the pinned SDK headers, both from `wellfed-amebad-sdk`. The
//! routines both cores link (flash park, hooks, peer, OTA trial) live in
//! `wellfed-amebad-common` and are re-exported here.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The ports' public types carry SDK types, so the SDK crate is part of this API.
pub use wellfed_amebad_common::{cell, flash, trial};
pub use wellfed_amebad_sdk as sdk;

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-amebad target builds are KM4-only; KM0 links wellfed-amebad-common");

pub mod leds;
pub mod startup;
pub mod stopped;

pub mod arming;
pub mod audio;
pub mod board;
pub mod clock;
pub mod confirm;
pub mod network;
pub mod settings;
pub mod stream;
pub mod supervise;

mod active;
pub mod config;
pub mod events;
pub mod health;
pub mod net_status;
pub mod owner;
pub mod provider;
pub mod transport;
pub mod updater;
pub mod watch;
pub mod watchdog;
