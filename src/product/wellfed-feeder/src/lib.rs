//! The feeder's product policy: what the owner sees the device do, on top of
//! the SDK's mechanisms ([product architecture]). It knows no hardware and
//! keeps no clock, so the firmware steps it and the host tests it.
//!
//! [product architecture]: ../../../docs/firmware/product-architecture.md
#![no_std]
#![deny(unsafe_code)]

pub mod api;
pub mod feed;
pub mod mailbox;
pub mod pairing;
pub mod panel;
pub mod setup;
pub mod store;
pub mod update;
