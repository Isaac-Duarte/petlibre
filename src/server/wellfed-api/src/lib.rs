//! The owner's server HTTP API ([API design]): every request and response
//! body under `/api/v1`, its error codes and its OpenAPI schemas.
//!
//! The server serializes these, and the app (through generated bindings)
//! and tests deserialize them, so the crate builds for every target and
//! carries no server dependency. The web UI's types come from the
//! `openapi.json` the server generates from them.
//!
//! Fields are `snake_case`, IDs UUIDv7, and times RFC 3339. Feeder keys and
//! fingerprints travel as lowercase hex.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod auth;
mod error;
mod events;
mod feeders;
mod stream;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;
use uuid::Uuid;

pub use auth::{Credentials, Role, SetupState, User};
pub use error::{ErrorCode, FeederRefusal, Problem};
pub use events::{
    ClientRole, EnrollPath, EventBody, EventKind, EventPage, EventQuery, FeedOutcome, FeedReason,
    FeederEvent, Gap, Source,
};
pub use feeders::{
    AdoptFeeder, Connection, Feeder, FeederDetail, FeederStatus, LockRequest, Power, Settings,
    SettingsPatch, StatusSnapshot, WifiSource,
};
pub use stream::{FeederSample, MetricKind, SampleValue, StreamEvent, StreamQuery};

/// The prefix every resource lives under.
pub const PREFIX: &str = "/api/v1";

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(transparent)]
#[schema(value_type = String, format = Uuid)]
pub struct FeederId(pub Uuid);

impl fmt::Display for FeederId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(transparent)]
#[schema(value_type = String, format = Uuid)]
pub struct UserId(pub Uuid);

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

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
        wellfed_core::hex::Lower(&self.0).fmt(f)
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
        wellfed_core::hex::decode_array(text.as_bytes())
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom(format!("not {N} bytes of lowercase hex")))
    }
}

/// Liveness, without authentication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Health {
    pub version: String,
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
