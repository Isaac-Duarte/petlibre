//! A simulated feeder that serves the real device API over TCP, for the
//! server, the app and the CLI to work against without hardware.
//!
//! It runs the feeder's own code: `wellfed-feeder`'s API server, panel
//! and pairing policy, and `wellfed-core`'s stores on a flash image laid
//! out like the feeder's storage region. What it fakes is what the board
//! provides: the buttons (the operator's [`console`]), the supply and
//! signal it samples, and the time. Nothing moves: a feed is refused, as
//! on the current firmware, until the feeding owner lands.
//!
//! - [`flash`]: the storage region, in memory or in a file.
//! - [`device`]: the server's view of the feeder, over the stores.
//! - [`sim`]: one feeder, stepped in the firmware loop's order.
//! - [`serve`]: the sockets, and the loop that drives it.

#![forbid(unsafe_code)]

pub mod console;
pub mod device;
pub mod flash;
pub mod serve;
pub mod sim;
