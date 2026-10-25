//! EXPERIMENTAL product firmware for the PLAF103 on the Wellfed SDK: the
//! one `feeder` firmware every owner runs ([product architecture]). It is the
//! PLAF103's glue between the SDK and the product policy in
//! `wellfed-feeder`, and grows owner by owner.
//!
//! So far it runs the panel: the button service and one loop task that keeps
//! the child lock, shows it on the lock indicator, and takes signed
//! lock changes from the network. The loop also holds the feeder's identity
//! key pair and the pairing policy: a Reset hold clears the keys, and a Lock
//! and Mic hold starts BLE setup mode, where a new owner's phone gives the
//! feeder Wi-Fi and its key. The loop also serves the device API
//! (`api`): Noise sessions over TCP, advertised over mDNS, with the
//! event history and live samples. The loop also steps the feeding owner
//! (`feeding`), which holds the motor and emitters stopped and supervised
//! and runs the feeds the device API submits. The `entropy` variant streams raw noise
//! samples for the host's entropy estimate, and the `owner-stall` variant stalls
//! the loop on purpose, for the bench to watch the owner's beat lapse.
//!
//! [product architecture]: ../../../../docs/firmware/product-architecture.md
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-feeder target builds are KM4-only");

pub mod api;
#[cfg(target_os = "none")]
mod app;
pub mod entropy;
pub mod feeding;
pub mod keys;
pub mod owner;
pub mod setup;
