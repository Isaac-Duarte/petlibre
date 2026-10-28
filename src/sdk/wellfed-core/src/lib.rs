//! Hardware-independent replacement-firmware logic.
//!
//! States, faults and reasons are Rust enums with fixed discriminants: a
//! record or report carries a variant's number (`code`, or `byte` for a
//! one-byte flash field), and a record's byte becomes a variant only through
//! `from_byte`, where an unknown value is the store's corruption error. Time
//! values are wrapping `u32` milliseconds; a forward distance of 2^31 or more
//! is ambiguous and treated as a clock failure.
#![no_std]
#![deny(unsafe_code)]

// The SDK's crates declare their UART record words (`stage=`, `reason=`,
// fields the host tools parse) with the session crate's macro: the printed
// text is exactly the declared word, so a record's bytes do not depend on
// how the firmware names the variant.
pub use wellfed_session::named_enum;

pub mod adc_scale;
pub mod ble_setup;
pub mod buttons;
pub mod clip;
pub mod clock;
pub mod desiccant;
pub mod feed;
pub mod flash_coord;
pub mod indicator;
pub mod liveness;
pub mod log_tail;
pub mod mdns;
pub mod ota;
pub mod record;
pub mod rng;
pub mod schedule;
pub mod settings;
pub mod store;
pub mod supply;
pub mod tone;

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

/// Milliseconds from `now` until `deadline`: 0 once it has passed. A plain
/// wrapping difference turns a deadline just behind `now` into a wait of
/// nearly the whole range.
#[inline]
pub const fn until(now: u32, deadline: u32) -> u32 {
    if deadline_passed(now, deadline) {
        0
    } else {
        deadline.wrapping_sub(now)
    }
}
