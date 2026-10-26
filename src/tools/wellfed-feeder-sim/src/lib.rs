//! A simulated feeder that serves the real device API over TCP, for the
//! server, the app and the CLI to work against without hardware.
//!
//! It runs the feeder's own code: the product's loop pass
//! (`wellfed_feeder::feeder`, the device API's driver and server, the
//! panel, pairing and setup mode, the event recorder) and the product's
//! stores on a flash image laid out like the feeder's storage region. What
//! it fakes is what the board provides ([`platform`]): the buttons (the
//! operator's [`console`]), the supply and signal it samples, the settings
//! writer's turn and the time. Nothing moves: it has no feeding owner, so it
//! does not claim the feed capability and refuses feeds. It takes updates
//! as the feeder does, through the firmware's own update stage on two
//! simulated banks, with the loader's choice and KM0's trial around them
//! ([`banks`]).
//!
//! - [`flash`]: the storage region, in memory or in a file.
//! - [`banks`]: the two banks, the loader and the boot trial.
//! - [`platform`]: the loop's platform, over the stores and host sockets.
//! - [`sim`]: one feeder: the loop's pass, the writer's and update stage's
//!   turns.
//! - [`serve`]: the listener, and the loop that drives it.
//! - `harness` (the `harness` feature, for tests only): a simulator on a
//!   test's `LocalSet`, and a relay that can stall like a feeder that lost
//!   power.

#![forbid(unsafe_code)]

pub mod banks;
pub mod console;
pub mod flash;
#[cfg(feature = "harness")]
pub mod harness;
pub mod platform;
pub mod serve;
pub mod sim;
