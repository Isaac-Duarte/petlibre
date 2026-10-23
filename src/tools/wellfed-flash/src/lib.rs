//! UART flash reads, region-scoped development updates, give-ways and
//! original-stock recovery for the PLAF103 research unit.
//!
//! Ported from the Python clients formerly under `src/tools/flash/`, keeping
//! their report, plan and journal formats. Every writer keeps the pinned helper,
//! baseline and marker-last sequencing, and stops terminally without retry.
//! Host tests and emulation establish none of its hardware behavior.

#![forbid(unsafe_code)]

pub mod error;
pub mod evidence;
pub mod link;
pub mod ota;
pub mod writers;

#[cfg(test)]
mod tests;

pub use error::{Error, Result};
