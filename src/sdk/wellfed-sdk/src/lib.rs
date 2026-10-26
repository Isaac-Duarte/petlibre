//! EXPERIMENTAL Wellfed SDK for the PLAF103: the runtime a firmware runs
//! inside, and the API it builds on.
//!
//! The runtime (target builds only) owns the KM4 entry, startup, the
//! watchdog feeder, the network join, the confirmation, the settings writer's
//! turn and the update stage. None of those can be skipped by a firmware: the
//! update stage runs beside the firmware on both banks, and a firmware that
//! hangs or fails its trial is rolled back by KM0 to bank 0's.
//!
//! The device's record stores (settings, Wi-Fi credentials, keys, events)
//! are the product's schema, so the SDK does not know them: the runtime
//! steps the writer the firmware links (`wellfed_store_step`), and the
//! Wi-Fi join asks the same crate for the stored credentials
//! (`wellfed_wifi_select`). `wellfed-feeder-runtime` defines both, and
//! every PLAF103 firmware links it, so a firmware swap keeps the owner's
//! data.
//!
//! A firmware is a crate that defines its start hook:
//!
//! ```ignore
//! use wellfed_feeder_runtime as _;
//!
//! fn start() {
//!     // Create the firmware's tasks with `wellfed_sdk::spawn`.
//!     wellfed_sdk::startup_done(true);
//! }
//! wellfed_sdk::firmware!(start);
//! ```
//!
//! Host builds test the Wi-Fi probe against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-sdk target builds are KM4-only");

pub mod buttons;
#[cfg(target_os = "none")]
mod runtime;
mod slot;
pub mod storage;
mod tasks;
pub mod wifi;

#[cfg(target_os = "none")]
pub use runtime::{
    JournalSlots, SdkHooks, SdkStorage, claim_adc, flash_authority, request_reset, spawn,
    startup_done, storage_armed, watchdog_open,
};
pub use slot::{Slot, SlotGuard};
pub use tasks::FIRMWARE_TASKS;

/// The ports a firmware drives (indicators, audio, inputs). Startup,
/// supervision, flash and updates stay the runtime's.
pub use wellfed_hal as hal;

/// Defines the firmware's start hook. The runtime calls `$start` once, in
/// the normal role, from its startup task after the startup checks passed
/// and the runtime's service task was created. `$start` creates the firmware's
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
