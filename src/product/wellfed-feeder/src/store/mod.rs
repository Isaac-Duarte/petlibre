//! The product's records on NOR: device settings, Wi-Fi credentials, the
//! client keys and the time settings, each a dual-slot store (`wellfed_core::store::slot`: two
//! alternating erase units under one torn-write rule, with the store's own
//! fields and recovery policy), and the event history in a ring of units
//! ([`events`]). Each keeps its sectors in the board's map and its on-flash
//! layout as the firmware has always written it.
//!
//! The ring is not a dual-slot store: it appends many short records with no
//! commit marker, and a torn one simply fails its checksum and is skipped,
//! so it shares only the slot layer's I/O trait, checksum and field
//! helpers.

pub mod clients;
pub mod events;
pub mod net;
pub mod settings;
pub mod time;
