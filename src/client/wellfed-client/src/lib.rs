//! The client side of the feeder's protocols ([API design]), sans-IO: the
//! caller moves bytes over BLE or a socket and this crate decides what they
//! say. The Android app (through generated bindings) and the owner's
//! `wellfed` CLI share it, so the two cannot disagree about the wire.
//!
//! It holds FW45's BLE setup session ([`setup`]) and FW47's device API
//! sessions and invite redemption ([`api`]).
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

pub mod api;
pub mod setup;
pub mod time;
