//! The feeder's record stores on the Wellfed SDK runtime: the glue between
//! the product's schema (`wellfed_feeder::store`) and the SDK's tasks.
//!
//! The SDK runtime runs a settings writer in its service task and joins
//! Wi-Fi from its C task on every firmware, but the stores they read and
//! write are the product's, which the SDK may not depend on. So the runtime
//! calls two hooks this crate defines, and every PLAF103 firmware links it
//! (`use wellfed_feeder_runtime as _;`), which keeps the owner's settings,
//! keys and history across a firmware swap:
//! - `wellfed_store_step`, the service task's settings stage: the
//!   [`writer`], which alone writes the settings, Wi-Fi, client and event
//!   stores;
//! - `wellfed_wifi_select` and `wellfed_wifi_outcome`, the C Wi-Fi task's
//!   choice between stored and build credentials ([`network`]).
//!
//! The rest carries work between the loop task and the writer, each a
//! one-slot or bounded mailbox: client changes and the identity ([`keys`]),
//! Wi-Fi saves ([`network`]), the time settings ([`time`]), the portion
//! calibration ([`calibration`]), the feeding plans ([`plans`]), the event
//! history ([`events`]) and its fill for the statistics ([`stats`]), and
//! the latest feed's trace on its way to storage ([`traces`]).
//! [`stress`]
//! is the settings store's power-cut probe, kept for its host tests.
//!
//! Host builds test everything against the SDK fake.
#![cfg_attr(target_os = "none", no_std)]
#![allow(clippy::missing_safety_doc)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-feeder-runtime target builds are KM4-only");

pub mod calibration;
pub mod events;
pub mod keys;
pub mod network;
pub mod plans;
pub mod stats;
pub mod stress;
pub mod time;
pub mod traces;
pub mod writer;
