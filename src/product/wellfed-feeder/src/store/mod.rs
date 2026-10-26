//! The product's records on NOR: device settings, Wi-Fi credentials and the
//! client keys, each in two alternating erase units over the SDK's record
//! layer (`wellfed_core::store::slot`), and the event history in a ring of
//! units ([`events`]). Each keeps its sectors in the board's map and its
//! on-flash layout as the firmware has always written it.

pub mod clients;
pub mod events;
pub mod net;
pub mod settings;
