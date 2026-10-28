//! What stock PLAF103 1.1.15 speaks, and only what installing from it and
//! rolling back to it need (src/tools/README.md): its MQTT
//! protocol, the broker it connects to, the setup frame, the image server it
//! downloads a replacement from, and the ledger that sends each install
//! command at most once. The replacement firmware speaks no MQTT; its
//! updates and status go over its device API.
//!
//! The `wellfed-stock` binary is the bench's server over it
//! (docs/development/stock-server.md): `serve` runs the broker, the session
//! with stock and the loopback API in [`api`]; `ota-images` the image
//! server; `network` the feeder-only gateway redirect in [`network`];
//! `plans` stock's stored feeding plans from a flash read ([`plans`]).

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod build_info;
pub mod config;
pub mod controller;
pub mod enroll;
pub mod mqtt;
pub mod neighbour;
pub mod network;
pub mod ota;
pub mod plans;
pub mod protocol;
pub mod setup;
pub mod storage;
