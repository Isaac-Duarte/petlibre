//! `--ui`: a read-only page of the feeder, bench progress, the integration
//! suite's runs, and the evidence sessions.
//!
//! The views read the status files, the evidence directories and the local
//! server's status only. Nothing here opens a serial port, so it cannot
//! disturb an update, a dump or a capture, and nothing writes. The views
//! always build, so their tests and TypeScript bindings run in the
//! contributor gate; the server and its embedded web app need the `ui`
//! feature and a built `src/web/apps/bench/dist` (`make ui`).

#[cfg(any(feature = "ui", test))]
mod feeder;
#[cfg(any(feature = "ui", test))]
mod runs;
#[cfg(any(feature = "ui", test))]
mod views;

#[cfg(feature = "ui")]
mod serve;
#[cfg(feature = "ui")]
pub use serve::{Listen, start};

#[cfg(test)]
mod bindings;
