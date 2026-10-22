//! EXPERIMENTAL AmebaD routines that both PLAF103 cores link.
//!
//! The KM0 image contains only this crate: the RAM flash park, the SDK flash
//! hooks, the park peer and the OTA trial step. `petlibre-amebad` re-exports
//! these modules for the KM4 ports. Host builds run against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

// The KM4 provider fault callback calls the SDK's output guard.
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

pub mod flash;
pub mod trial;
