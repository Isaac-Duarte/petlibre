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

mod audit;
mod auth;
mod backups;
mod clients;
mod commands;
mod diagnostics;
mod error;
mod events;
mod feeders;
mod foods;
mod health;
mod household;
mod installs;
mod pets;
mod releases;
mod samples;
mod stock;
mod stream;
mod tokens;
mod users;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

pub use audit::{AuditAction, AuditEntry, AuditPage, LoginFailure};
pub use auth::{Credentials, PasswordChange, Role, Setup, SetupState, User};
pub use backups::{Backup, BackupPage};
pub use clients::{
    ClientOwner, FeederClient, KeyRemoval, NewPhoneInvite, PhoneInvite, PhoneKey, PlayRequest,
    Removal, RevokeAttempt, RevokeReason, Revoking, TuneName, WifiChange, WifiCredentials,
};
pub use commands::{
    Command, CommandKind, CommandOutcome, CommandPage, CommandQuery, CommandState, FeedRequest,
};
pub use diagnostics::{FeedTrace, FeederStats, LogEvent, StatsRead, TraceQuery, TraceRecord};
pub use error::{ErrorCode, FeederRefusal, Problem};
pub use events::{EventPage, EventQuery, FeederEvent, Gap};
pub use feeders::{AdoptFeeder, Connection, Feeder, FeederDetail, LockRequest, StatusSnapshot};
pub use foods::{
    FeederFood, Food, FoodChoice, FoodHistoryPage, FoodPatch, FoodPeriod, GRAMS_PER_CUP, NewFood,
    NewRefill, Refill, RefillPage,
};
pub use health::{FeederLink, Health, Job, JobOutcome, JobRun, ServerStatus, StoreState};
pub use household::{Household, HouseholdPatch, TimeFormat, Units};
pub use installs::{Install, InstallFailure, InstallRequest, InstallState, InstallTarget};
// The feeder's own values are the model's, served as they are; the HTTP
// API adds only what the server knows (when it received them, its own ids).
pub use wellfed_model::{
    Blocked, ClientRole, CurrentUnit, EnrollPath, EventBody, EventKind, FeedOutcome, FeedReason,
    Food as FoodStats, Generation, Hex, MetricKind, Power, Readiness, SampleValue, Settings,
    SettingsPatch, Source, Stats, StatsSection, Status as FeederStatus, Trace, TraceEntry,
    TraceSummary, WifiSource,
};
pub use pets::{NewPet, NewWeight, Pet, PetLink, PetPatch, Species, Weight, WeightPage};
pub use releases::{
    AssetName, RefusedAsset, Release, ReleaseCheck, ReleasePage, ReleaseQuery, ReleaseRefusal,
    ReleaseSigner, ReleaseSource,
};
pub use samples::{
    HourSample, RawSample, Resolution, SAMPLE_NUMBER_VERSION, SamplePage, SampleQuery,
    sample_number,
};
pub use stock::{
    MANIFEST_LEN, STATEMENT_LEN, StockBackup, StockBackupState, StockFeeder, StockRestore,
    StockSource, StockState,
};
pub use stream::{FeederSample, StreamEvent, StreamQuery};
pub use tokens::{BEARER, CreatedToken, NewToken, Token, TokenId};
pub use users::{NewUser, UserPatch};

/// The prefix every resource lives under.
pub const PREFIX: &str = "/api/v1";

/// The cookie a login sets: an opaque session token, `HttpOnly` and
/// `SameSite=Strict`. A non-browser client sends it back as it came.
pub const SESSION_COOKIE: &str = "wellfed_session";

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

/// Ids the server gives: UUIDv7s that serialize as their strings.
macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
            ToSchema,
        )]
        #[serde(transparent)]
        #[schema(value_type = String, format = Uuid)]
        pub struct $name(pub Uuid);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

ids!(
    UserId,
    PetId,
    /// One entry of a pet's weight log.
    WeightId,
    FoodId,
    RefillId,
    InstallId,
);

/// A page of a list that grows: the previous page's `next`, and a size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// The previous page's `next`, as it came.
    pub after: Option<String>,
    /// At most this many (1-1000, default 100).
    pub limit: Option<u32>,
}

/// A field that is present is `Some`, even when it is `null`: absent
/// leaves a value alone, `null` clears it.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
