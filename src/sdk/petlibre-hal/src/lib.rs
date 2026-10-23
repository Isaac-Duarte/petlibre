//! EXPERIMENTAL AmebaD ports for the PLAF103 research board.
//!
//! Host builds run against [`sdk::fake`]; KM4 target builds need the `sdk`
//! feature and the pinned SDK headers, both from `petlibre-amebad-sys`. The
//! routines both cores link (flash park, hooks, peer, OTA trial) live in
//! `petlibre-hal-common` and are re-exported here.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The ports' public types carry SDK types, so the SDK crate is part of this API.
pub use petlibre_amebad_sys as sdk;
pub use petlibre_hal_common::{cell, flash, trial};

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("petlibre-hal target builds are KM4-only; KM0 links petlibre-hal-common");

pub mod leds;
pub mod startup;
pub mod stopped;

pub mod adc;
pub mod audio;
// FW34: compiled for host tests, and on the target only for a Bluetooth
// firmware, whose image links the C peripheral it calls.
#[cfg(any(not(target_os = "none"), feature = "bluetooth"))]
pub mod ble;
pub mod board;
pub mod buttons;
pub mod clock;
pub mod network;
pub mod settings;
pub mod stream;

pub mod flash_lock;
pub mod health;
pub mod net_status;
pub mod ota;
pub mod owner;
pub mod power;
pub mod supervision;
pub mod transport;
