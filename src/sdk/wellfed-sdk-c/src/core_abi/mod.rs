//! C ABI for `wellfed-core`, matching the original C firmware headers.
//!
//! Enumeration ABI: every enumeration-typed field, argument and return value
//! (`MotorCommand`, `FeedState`, `FeedReason`, `BowlMode`, `Direction`,
//! `FlashPhase`, ...) is a 32-bit unsigned integer (`#[repr(transparent)]`
//! `u32`). C declarations MUST use `uint32_t` plus `#define`/`enum` constants
//! for the values, never an `enum` type: the pinned AmebaD GCC does not pass
//! `-fno-short-enums`, so a C `enum` would be one byte and silently change
//! struct layouts (for example `MotorGate` would shrink from 48 bytes). The
//! SDK's flags are fixed by its prebuilt libraries and are not changed here.
//! The assertions in `layout.rs` lock every exported
//! `#[repr(C)]` struct's size, alignment and enum-bearing field offsets for
//! both 32-bit firmware and 64-bit host builds.

mod adc_scale;
mod diagnostic;
mod feed_controller;
mod feed_journal;
mod feed_progress;
mod feedback_events;
mod flash_coord;
mod motor_gate;
mod sensor_inputs;
mod tick_clock;

pub use adc_scale::*;
pub use diagnostic::*;
pub use feed_controller::*;
pub use feed_journal::*;
pub use feed_progress::*;
pub use feedback_events::*;
pub use flash_coord::*;
pub use motor_gate::*;
pub use sensor_inputs::*;
pub use tick_clock::*;

#[allow(dead_code)]
mod layout;

/// Store `value` through `out` when both are present.
fn store<T>(out: Option<&mut T>, value: Option<T>) -> bool {
    match (out, value) {
        (Some(out), Some(value)) => {
            *out = value;
            true
        }
        _ => false,
    }
}
