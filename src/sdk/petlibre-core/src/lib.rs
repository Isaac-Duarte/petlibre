//! Hardware-independent replacement-firmware logic.
//!
//! Every type keeps the C header's field order and `#[repr(C)]` layout so the
//! SDK-facing C ABI can expose it unchanged. Enumerations are transparent
//! integers: values read from flash or supplied over the ABI may be invalid,
//! and each operation validates them exactly where the original design did.
//! Time values are wrapping `u32` milliseconds; a forward distance of 2^31 or
//! more is ambiguous and treated as a clock failure.
#![no_std]
#![deny(unsafe_code)]

/// Declare a C-compatible enumeration that tolerates out-of-range values.
///
/// In the C ABI each such type is a 32-bit unsigned integer: C declarations
/// must use `uint32_t`, not an `enum` type, because the AmebaD GCC may lay out
/// a C `enum` in one byte (no `-fno-short-enums`).
///
/// Exported for the AmebaD port crates, so every C-facing enumeration in the
/// firmware has the same representation and derives.
#[macro_export]
macro_rules! c_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            $($(#[$vmeta])* pub const $variant: Self = Self($value);)+

            /// The low byte, as the one-byte fields of the NOR records store
            /// the value. Every declared variant fits.
            #[allow(clippy::cast_possible_truncation, reason = "record fields are one byte wide")]
            pub const fn low_byte(self) -> u8 {
                self.0 as u8
            }
        }
    };
}

/// Declare a fieldless enumeration whose variants print as fixed words.
///
/// For the UART record fields (`stage=`, `reason=`) the host tools parse:
/// `as_str` and `Display` give exactly the declared text, so a record's
/// bytes do not depend on how the firmware names the variant.
#[macro_export]
macro_rules! named_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        $vis enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(self.as_str(), f)
            }
        }
    };
}

pub mod adc_scale;
pub mod base64;
pub mod ble_setup;
pub mod buttons;
pub mod clock;
pub mod feed;
pub mod flash_coord;
pub mod hex;
pub mod noise;
pub mod ota;
pub mod record;
pub mod request_auth;
pub mod rng;
pub mod setup_code;
pub mod speaker_stream;
pub mod store;
pub mod tone;
pub mod transport;

/// Distances at or beyond this are ambiguous for wrapping millisecond clocks.
pub const HALF_RANGE: u32 = 0x8000_0000;

/// True for a nonzero interval that is unambiguous on a wrapping clock.
pub const fn interval_valid(value: u32) -> bool {
    value > 0 && value < HALF_RANGE
}

/// `now` is at or past `deadline` on a wrapping clock: the deadline lies at
/// most half the range behind. A deadline further ahead than that reads as
/// already passed, so deadlines must be set less than half the range out.
#[inline]
pub const fn deadline_passed(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) < HALF_RANGE
}
