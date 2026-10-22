//! EXPERIMENTAL AmebaD ports for the PLAF103 research board.
//!
//! Host builds run against [`sdk::fake`]; KM4 target builds need the `sdk`
//! feature and the pinned SDK headers, both from `petlibre-amebad-sdk`. KM0
//! builds contain only the SDK-independent flash-park and hook routines.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The ports' public types carry SDK types, so the SDK crate is part of this API.
pub use petlibre_amebad_sdk as sdk;

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

#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod leds;
pub mod startup;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod stopped;

#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod arming;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod board;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod confirm;

#[cfg(any(not(target_os = "none"), feature = "km4"))]
mod active;
#[cfg(all(target_os = "none", feature = "km4"))]
pub mod app;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod config;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod events;
pub mod flash;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod health;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod net_status;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod owner;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod probes;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod transport;
pub mod trial;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod updater;
#[cfg(any(not(target_os = "none"), feature = "km4"))]
pub mod watchdog;
