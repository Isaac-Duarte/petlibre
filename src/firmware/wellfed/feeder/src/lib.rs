//! EXPERIMENTAL product firmware for the PLAF103 on the Wellfed SDK: the
//! one `feeder` firmware every owner runs ([product architecture]). The
//! product itself is `wellfed-feeder`'s loop (`wellfed_feeder::feeder`):
//! the panel and its child lock, pairing and BLE setup mode, the device
//! API with its sockets and mDNS, and the event recorder, one pass at a
//! time. This crate is the PLAF103's wiring for it:
//! - [`platform`]: the loop's `Platform` on this board: the lock indicator,
//!   setup mode's BLE link and speaker, lwIP's sockets, and the runtime's
//!   mailboxes, clock, interlock and update mailbox;
//! - [`feeding`]: the feeding owner with this board's limits, which the loop
//!   task steps before each pass and the device API reaches as the pass's
//!   `Feeds`; it takes turns with the Wi-Fi task's joins (an association
//!   stalls every task, so neither starts while the other runs);
//! - the start hook and the loop task (`app`), beside the button service.
//!
//! The `entropy` variant streams raw noise samples for the host's entropy
//! estimate, the `owner-stall` variant stalls the loop on purpose, for the
//! bench to watch the owner's beat lapse, and the `gap-probe` variant
//! measures how long the loop and a priority-4 task wait across Wi-Fi
//! rejoins (`gaps`), with feeds refused.
//!
//! [product architecture]: ../../../../docs/firmware/product-architecture.md
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-feeder target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
pub mod entropy;
pub mod feeding;
pub mod gaps;
pub mod platform;
