//! The device API's values as every host works with them: owned, with no
//! lifetimes or wire buffers, so the client's `Connection` can hand them
//! out, the server can store and serve them, and the app can take them
//! through generated bindings ([API design]).
//!
//! `wellfed-proto` is the wire: borrowed, `no_std` and positional, shaped
//! for the feeder. This crate is what a host means by the same values, and
//! [`convert`] is the one place the two meet, so the client, the server
//! and the CLI cannot read a field differently.
//!
//! The JSON forms are the HTTP API's (`wellfed-api` serves these types
//! as they are, and its OpenAPI schemas come from their derives): fields
//! `snake_case`, times RFC 3339, keys and fingerprints lowercase hex.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

pub mod convert;
mod event;
mod request;
mod session;
pub mod setup;
mod status;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;

pub use event::{
    ClientRole, EnrollPath, Event, EventBody, EventKind, FeedOutcome, FeedReason, Gap, MetricKind,
    Pushed, Sample, SampleValue, Source,
};
pub use request::{Answer, Client, Invite, Op, Refusal, TimeSync, Tune, Wifi};
pub use session::{Capabilities, Hello, Joined, OpenRefusal};
pub use status::{Power, Settings, SettingsPatch, Status, WifiSource};

/// A feeder store's generation: a write names the one it read, and the
/// feeder refuses it once the store has moved on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = u32)]
pub struct Generation(pub u32);

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Bytes that travel as lowercase hex: a feeder's 32-byte identity key or a
/// client's 4-byte fingerprint. Fields of this type carry
/// `#[schema(value_type = String)]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hex<const N: usize>(pub [u8; N]);

impl<const N: usize> fmt::Display for Hex<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        wellfed_session::hex::Lower(&self.0).fmt(f)
    }
}

impl<const N: usize> fmt::Debug for Hex<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<const N: usize> Serialize for Hex<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de, const N: usize> Deserialize<'de> for Hex<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        wellfed_session::hex::decode_array(text.as_bytes())
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom(format!("not {N} bytes of lowercase hex")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_lowercase_and_exact_length() {
        let json = serde_json::to_string(&Hex([0xab, 0x01])).unwrap();
        assert_eq!(json, "\"ab01\"");
        assert_eq!(
            serde_json::from_str::<Hex<2>>(&json).unwrap(),
            Hex([0xab, 0x01])
        );
        assert!(serde_json::from_str::<Hex<2>>("\"AB01\"").is_err());
        assert!(serde_json::from_str::<Hex<2>>("\"ab\"").is_err());
    }
}
