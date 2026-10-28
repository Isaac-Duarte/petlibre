//! The product's records on NOR: device settings, Wi-Fi credentials, the
//! client keys, the time settings, the lifetime statistics, the portion
//! calibration and the feeding plans, each a dual-slot store
//! (`wellfed_core::store::slot`: two alternating erase units under one
//! torn-write rule, with the store's own fields and recovery policy), and
//! the event history and the per-feed traces, each in a ring of units
//! ([`events`], [`traces`]). Each keeps its sectors in the board's map and
//! its on-flash layout as the firmware has always written it.
//!
//! A ring is not a dual-slot store: it appends records with no commit
//! marker, and a torn one simply fails its checksum and is skipped, so it
//! shares only the slot layer's I/O trait, checksum and field helpers.

pub mod calibration;
pub mod clients;
pub mod events;
pub mod net;
pub mod plans;
pub mod settings;
pub mod stats;
pub mod time;
pub mod traces;
