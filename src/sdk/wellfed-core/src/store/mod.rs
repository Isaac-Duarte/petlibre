//! Records kept on NOR: the feed journal, device settings, Wi-Fi
//! credentials, the random seed, the client keys and the update store, each
//! in two alternating erase units over the shared record layer in [`slot`], and
//! the event history in a ring of units ([`events`]).

pub mod clients;
pub mod events;
pub mod feed_journal;
pub mod net;
pub mod seed;
pub mod settings;
pub mod slot;
pub mod update;
