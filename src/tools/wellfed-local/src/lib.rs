//! The bench's local server for the PLAF103: a thin binary over
//! `wellfed-stock` for the stock install path, plus the replacement
//! firmware's MQTT transport, until both move to single-purpose crates
//! (docs/development/tools-roadmap.md).

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod build_info;
pub mod config;
pub mod neighbour;
pub mod network;
pub mod ota;
pub mod replacement;
