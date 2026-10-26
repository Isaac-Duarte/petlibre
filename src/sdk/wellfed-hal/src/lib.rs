//! EXPERIMENTAL AmebaD ports for the PLAF103 research board.
//!
//! Host builds run against [`sdk::fake`]; KM4 target builds need the `sdk`
//! feature and the pinned SDK headers, both from `wellfed-amebad-sys`. The
//! routines both cores link (flash park, hooks, peer, OTA trial) live in
//! `wellfed-hal-common` and are re-exported here.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The ports' public types carry SDK types, so the SDK crate is part of this API.
pub use wellfed_amebad_sys as sdk;
// The board the ports drive: its pins and storage sector map.
pub use wellfed_board_plaf103 as plaf103;
pub use wellfed_hal_common::{cell, flash, trial};

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-hal target builds are KM4-only; KM0 links wellfed-hal-common");

pub mod leds;
pub mod startup;
pub mod stopped;

pub mod adc;
pub mod audio;
// Compiled for host tests, and on the target only for a Bluetooth
// firmware, whose image links the C peripheral it calls.
#[cfg(any(not(target_os = "none"), feature = "bluetooth"))]
pub mod ble;
pub mod board;
pub mod bringup;
pub mod buttons;
pub mod clock;
pub mod entropy;
pub mod events;
pub mod keys;
pub mod link;
pub mod network;
pub mod panel;
pub mod rng;
pub mod settings;

pub mod flash_lock;
pub mod health;
pub mod interlock;
pub mod net_status;
pub mod ota;
pub mod owner;
pub mod power;
pub mod supervision;
