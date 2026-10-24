//! EXPERIMENTAL product firmware for the PLAF103 on the Wellfed SDK: the
//! one `feeder` firmware every owner runs ([product architecture]). It is the
//! PLAF103's glue between the SDK and the product policy in
//! `wellfed-feeder`, and grows owner by owner.
//!
//! So far it runs the panel: the button service and one loop task that keeps
//! the child lock (FP11), shows it on the lock indicator, and takes signed
//! lock changes from the network. It never touches the motor or the infrared
//! emitters.
//!
//! [product architecture]: ../../../../docs/firmware/product-architecture.md
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-feeder target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
pub mod owner;
