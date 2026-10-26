//! The bench's local server for the PLAF103: a thin binary over
//! `wellfed-stock` for the stock install path, until that moves to
//! single-purpose crates (docs/development/tools-roadmap.md). The
//! replacement firmware speaks no MQTT; its updates go over its device API.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod build_info;
pub mod config;
pub mod neighbour;
pub mod network;
