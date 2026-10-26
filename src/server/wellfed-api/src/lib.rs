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

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub use auth::{Credentials, Role, SetupState, User};
pub use error::{ErrorCode, FeederRefusal, Problem};
pub use events::{EventPage, EventQuery, FeederEvent, Gap};
pub use feeders::{AdoptFeeder, Connection, Feeder, FeederDetail, LockRequest, StatusSnapshot};
// The feeder's own values are the model's, served as they are; the HTTP
// API adds only what the server knows (when it received them, its own ids).
pub use wellfed_model::{
    Blocked, ClientRole, EnrollPath, EventBody, EventKind, FeedOutcome, FeedReason, Generation,
    Hex, MetricKind, Power, Readiness, SampleValue, Settings, SettingsPatch, Source,
    Status as FeederStatus, WifiSource,
};
pub use stream::{FeederSample, StreamEvent, StreamQuery};

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

/// Liveness, without authentication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Health {
    pub version: String,
}
