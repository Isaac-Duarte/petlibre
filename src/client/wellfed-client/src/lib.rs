//! The client side of the feeder's protocols ([API design]), sans-IO: the
//! caller moves bytes over BLE or a socket and this crate decides what they
//! say. The Android app (through generated bindings) and the owner's
//! `wellfed` CLI share it, so the two cannot disagree about the wire.
//!
//! It holds the BLE setup session ([`setup`]), the device API's sessions
//! and invite redemption ([`api`]), and the whole session every client
//! runs over them ([`connection`]: keepalive, answers, the event cursor and
//! LAN recovery).
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

pub mod api;
pub mod connection;
pub mod setup;
pub mod time;
