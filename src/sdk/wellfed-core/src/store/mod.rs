//! Records kept on NOR that the SDK itself owns: the feed journal, the
//! random seed and the update store, each a dual-slot store ([`slot`]):
//! two alternating erase units under one torn-write rule, each store with
//! its own fields and recovery policy. The product's records (settings,
//! Wi-Fi credentials, client keys) are `wellfed_feeder::store`'s, in the
//! same dual-slot store, beside its event ring.

pub mod feed_journal;
pub mod handover;
pub mod seed;
pub mod slot;
pub mod update;
