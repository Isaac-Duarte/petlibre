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
//! A firmware is a crate that defines its start hook and its tasks:
//!
//! ```ignore
//! use wellfed_sdk::{Slot, Startup};
//!
//! #[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
//! fn start() -> Startup {
//!     wellfed_sdk::spawn(blink_task);
//!     Startup::Done
//! }
//!
//! static BLINKER: Slot<Blinker> = Slot::new();
//!
//! #[wellfed_sdk::task(name = "wellfed-blink", stack = 1024, priority = 2)]
//! fn blink_task() -> u32 {
//!     let Some(mut blinker) = BLINKER.enter_or_init(Blinker::default) else {
//!         return 0;
//!     };
//!     blinker.step()
//! }
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
    __started, JournalSlots, SdkHooks, SdkStorage, claim_adc, flash_authority, request_reset,
    spawn, startup_done, storage_armed, watchdog_open,
};
pub use slot::{Slot, SlotGuard};
pub use tasks::{FIRMWARE_TASKS, Task};

/// The ports a firmware drives (indicators, audio, inputs). Startup,
/// supervision, flash and updates stay the runtime's.
pub use wellfed_hal as hal;

pub use wellfed_hal::startup::{LogRecord, log};
pub use wellfed_sdk_macros::{Record, start, task};

/// What the start hook returns: whether the firmware's startup work
/// still holds the KM0 flash handshake (an optics or pulse probe). The
/// runtime does not confirm the build, sample power or join Wi-Fi before
/// that work ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    /// Nothing holds the handshake: the runtime may confirm once it is
    /// otherwise healthy. Reported as the hook returns, so it overrides a
    /// `startup_done(false)` one of its tasks made first.
    Done,
    /// A task holds it, and reports `startup_done` when it lets go.
    Pending,
}
