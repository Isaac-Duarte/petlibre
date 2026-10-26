//! The feeder's product policy: what the owner sees the device do, on top of
//! the SDK's mechanisms ([product architecture]). It knows no hardware and
//! keeps no clock, so the firmware steps it and the host tests it. Its
//! [`feeder`] is the loop itself, over a platform the firmware and the
//! simulator each provide.
//!
//! [product architecture]: ../../../docs/firmware/product-architecture.md
#![no_std]
#![deny(unsafe_code)]

#[cfg(feature = "conformance")]
pub mod conformance;
pub mod feed;
pub mod feeder;
pub mod panel;
pub mod stats;
pub mod store;
pub mod update;
