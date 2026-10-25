//! What stock PLAF103 1.1.15 speaks, and only what installing from it and
//! rolling back to it need (docs/development/tools-roadmap.md): its MQTT
//! protocol, the broker it connects to, the setup frame, the image server it
//! downloads a replacement from, and the ledger that sends each install
//! command at most once.
//!
//! The replacement firmware's MQTT transport is not stock and is not here.
//! Until it is deleted it shares the server's broker and MQTT session, so
//! `mqtt::broker::Roles` and `mqtt::Companion` let the caller add it beside
//! stock without this crate knowing its topics.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod build_info;
pub mod config;
pub mod controller;
pub mod mqtt;
pub mod ota;
pub mod protocol;
pub mod setup;
pub mod storage;
