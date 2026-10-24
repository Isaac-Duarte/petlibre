//! EXPERIMENTAL Wellfed SDK for the PLAF103: the runtime a firmware runs
//! inside, and the API it builds on.
//!
//! The runtime (target builds only) owns the KM4 entry, startup, the
//! watchdog feeder, network and health transport, the confirmation,
//! the settings writer and the updater and offer check. None of those
//! can be skipped by a firmware: bank 0 never runs firmware code, and a
//! firmware that hangs or fails its trial is rolled back by KM0.
//!
//! A firmware is a crate that defines its start hook:
//!
//! ```ignore
//! fn start() {
//!     // Create the firmware's tasks with `wellfed_sdk::spawn`.
//!     wellfed_sdk::startup_done(true);
//! }
//! wellfed_sdk::firmware!(start);
//! ```
//!
//! Host builds test the settings writer and the Wi-Fi probe against the SDK
//! fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-sdk target builds are KM4-only");

pub mod buttons;
#[cfg(target_os = "none")]
mod runtime;
pub mod settings;
mod slot;
pub mod storage;
mod tasks;
pub mod wifi;

#[cfg(target_os = "none")]
pub use runtime::{claim_adc, request_reset, spawn, startup_done, watchdog_open};
pub use slot::{Slot, SlotGuard};
pub use tasks::FIRMWARE_TASKS;

/// The ports a firmware drives (indicators, audio, inputs). Startup,
/// supervision, flash and updates stay the runtime's.
pub use wellfed_hal as hal;
// Every firmware archive carries the panic handler.
use wellfed_sdk_c as _;

/// Defines the firmware's start hook. The runtime calls `$start` once, in
/// the normal role, from its startup task after the startup checks passed
/// and the transport task was created. `$start` creates the firmware's
/// tasks and returns; it must not block.
#[macro_export]
macro_rules! firmware {
    ($start:path) => {
        #[cfg(target_os = "none")]
        #[unsafe(no_mangle)]
        extern "C" fn wellfed_firmware_start() {
            $start();
        }
    };
}
