//! The bench's local server for the PLAF103: the stock install path and the
//! replacement firmware's MQTT transport, until both move to single-purpose
//! crates (docs/development/tools-roadmap.md).

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod build_info;
pub mod config;
pub mod controller;
pub mod mqtt;
pub mod neighbour;
pub mod network;
pub mod ota;
pub mod protocol;
pub mod replacement;
pub mod stock_setup;
pub mod storage;
