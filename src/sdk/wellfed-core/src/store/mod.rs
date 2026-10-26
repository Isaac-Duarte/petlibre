//! Records kept on NOR that the SDK itself owns: the feed journal, the
//! random seed and the update store, each in two alternating erase units over
//! the shared record layer in [`slot`]. The product's records (settings,
//! Wi-Fi credentials, client keys and the event ring) are
//! `wellfed_feeder::store`'s, over the same layer.

pub mod feed_journal;
pub mod seed;
pub mod slot;
pub mod update;
