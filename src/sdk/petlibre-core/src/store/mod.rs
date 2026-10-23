//! Records kept on NOR: the feed journal, device settings and Wi-Fi
//! credentials, all over the shared two-slot record layer in [`slot`].

pub mod feed_journal;
pub mod net;
pub mod settings;
pub mod slot;
