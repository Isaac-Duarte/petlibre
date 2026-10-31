//! What stock PLAF103 1.1.15 speaks, and only what installing from it and
//! rolling back to it need (src/tools/README.md): its MQTT
//! protocol, the broker it connects to, the setup frame, the image server it
//! downloads a replacement from, and the ledger that sends each install
//! command at most once. The replacement firmware speaks no MQTT; its
//! updates and status go over its device API.
//!
//! [`server`] runs stock's side in the caller's process: the broker, the
//! session with stock and, optionally, the loopback API in [`api`]. [`ota`]'s
//! image server runs the same way. `wellfed install` and the Android app
//! (through `wellfed-install`) start both as tokio tasks they own and stop;
//! the `wellfed-stock` binary's subcommands are the same entry points.
//!
//! The binary is the bench's server over it
//! (docs/development/stock-server.md): `serve` runs [`server`]; `ota-images`
//! the image server; `network` the feeder-only gateway redirect in
//! `network`. The bench host's pieces (the binary, `network` and
//! `neighbour`) are behind the default `bench` feature, which an
//! installer leaves out.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod build_info;
pub mod clock;
pub mod config;
pub mod controller;
pub mod enroll;
pub mod mqtt;
#[cfg(feature = "bench")]
pub mod neighbour;
#[cfg(feature = "bench")]
pub mod network;
pub mod ota;
pub mod protocol;
pub mod server;
pub mod setup;
pub mod storage;
