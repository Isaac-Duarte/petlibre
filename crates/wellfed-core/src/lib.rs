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
macro_rules! c_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            $($(#[$vmeta])* pub const $variant: Self = Self($value);)+
        }
    };
}

pub mod adc_scale;
pub mod diagnostic;
pub mod feed_controller;
pub mod feed_journal;
pub mod feed_progress;
pub mod feed_storage;
pub mod feedback_events;
pub mod flash_coord;
pub mod motor_gate;
pub mod ota;
pub mod request_auth;
pub mod sensor_inputs;
pub mod settings_store;
pub mod storage;
pub mod tick_clock;
pub mod transport;
pub mod wall_clock;

/// Distances at or beyond this are ambiguous for wrapping millisecond clocks.
pub const HALF_RANGE: u32 = 0x8000_0000;

/// True for a nonzero interval that is unambiguous on a wrapping clock.
pub const fn interval_valid(value: u32) -> bool {
    value > 0 && value < HALF_RANGE
}
