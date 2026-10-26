//! The client side of the feeder's protocols ([API design]), sans-IO: the
//! caller moves bytes over BLE or a socket and this crate decides what they
//! say. The Android app (through generated bindings) and the owner's
//! `wellfed` CLI share it, so the two cannot disagree about the wire.
//!
//! It holds the BLE setup session ([`setup`]), the device API's sessions
//! and invite redemption ([`api`]), and the whole session every client
//! runs over them ([`connection`]: keepalive, answers, the event cursor and
//! LAN recovery), the update an owner pushes over it ([`update`]), and a
//! feed's trace read a page at a time ([`trace`]).
//! The consumer installer's steps from stock to a confirmed replacement,
//! stock's setup frame and the backup gate before promotion are
//! [`install`].
//!
//! Every public type is `wellfed-model`'s owned values or plain bytes (an
//! `Op` in; answers, pushed events, gaps and samples, welcomes, refusals and
//! setup replies out), from the lower-level [`api::Session`] the bench
//! drives up to [`connection::Connection`], so what crosses into the app's
//! bindings or the server has no lifetime tied to a frame buffer. The
//! wire's borrowed types stay inside the crate.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

pub mod api;
pub mod connection;
pub mod install;
pub mod setup;
pub mod time;
pub mod trace;
pub mod update;
