//! UART flash reads, region-scoped development updates, give-ways and
//! original-stock recovery for the PLAF103 research unit.
//!
//! A Rust port of the Python clients under `tools/flash/`, keeping their
//! report, plan and journal formats. Every writer keeps the pinned helper,
//! baseline and marker-last sequencing, and stops terminally without retry.
//! Host tests and emulation establish none of its hardware behavior.

pub mod development;
pub mod error;
pub mod evidence;
pub mod journal;
pub mod ota;
pub mod protocol;
pub mod read;
pub mod recovery;
pub mod review;
pub mod sector;
pub mod serial;
pub mod session;
pub mod status;
pub mod stock;
pub mod update;

#[cfg(test)]
mod tests;

pub use error::{Error, Result};
