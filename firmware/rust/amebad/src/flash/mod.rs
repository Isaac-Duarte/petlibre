//! Cross-core flash exclusion: the RAM parking substrate, the SDK flash
//! interposer and the KM0 peer that answers park requests.

pub mod hooks;
pub mod park;
pub mod peer;
