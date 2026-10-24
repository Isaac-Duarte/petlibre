//! The client side of the feeder's protocols ([API design]), sans-IO: the
//! caller moves bytes over BLE or a socket and this crate decides what they
//! say. The Android app (through generated bindings) and the owner's
//! `petlibre` CLI share it, so the two cannot disagree about the wire.
//!
//! So far it holds FW45's BLE setup session ([`setup`]); FW47's device API
//! sessions, invites and event sync follow.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

pub mod setup;
