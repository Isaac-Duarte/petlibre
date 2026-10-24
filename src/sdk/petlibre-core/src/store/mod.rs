//! Records kept on NOR: the feed journal, device settings, Wi-Fi
//! credentials, the random seed and the client keys, each in two
//! alternating erase units over the shared record layer in [`slot`].

pub mod clients;
pub mod feed_journal;
pub mod net;
pub mod seed;
pub mod settings;
pub mod slot;
