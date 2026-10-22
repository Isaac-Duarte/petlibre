//! EXPERIMENTAL AmebaD ports for the PLAF103 research board.
//!
//! Host builds run against [`sdk::fake`]; KM4 target builds need the `sdk`
//! feature and the pinned SDK headers, both from `wellfed-amebad-sdk`. The
//! routines both cores link (flash park, hooks, peer, OTA trial) live in
//! `wellfed-amebad-common` and are re-exported here.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The ports' public types carry SDK types, so the SDK crate is part of this API.
pub use wellfed_amebad_common::{flash, trial};
pub use wellfed_amebad_sdk as sdk;

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-amebad target builds are KM4-only; KM0 links wellfed-amebad-common");

/// Declare a C-compatible enumeration that tolerates out-of-range values.
macro_rules! c_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $value:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            $(pub const $variant: Self = Self($value);)+
        }
    };
}

pub mod leds;
pub mod startup;
pub mod stopped;

pub mod arming;
pub mod board;
pub mod confirm;

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
