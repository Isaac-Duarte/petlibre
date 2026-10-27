//! Errors as RFC 9457 `application/problem+json`, each with a typed code.

use wellfed_model::Refusal;

use crate::ReleaseRefusal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Every way a request fails. Clients branch on this, never on the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The first owner exists; setup is closed.
    SetupClosed,
    /// No valid session or token.
    Unauthenticated,
    BadCredentials,
    /// The account is past its expiry: it cannot log in and its sessions
    /// stop, until an owner extends or deletes it.
    AccountDisabled,
    /// This user's role may not do that.
    Forbidden,
    /// Only a login, not a token, may do that: making a token, setting a
    /// password or adding an account, so a leaked token cannot turn itself
    /// into a login or more tokens that outlive its revocation.
    SessionRequired,
    /// The change would leave the household without an owner.
    LastOwner,
    /// Another account has that name.
    NameTaken,
    /// A feeder's food history names the food, so deleting it would lose
    /// past feeds' calories.
    FoodInUse,
    /// A full-bag refill on a feeder with no food set: set its food first.
    NoCurrentFood,
    /// The zone is real, but no POSIX rule describes it (it changes
    /// irregularly), so a feeder could not keep it; `detail` names it.
    ZoneUnsupported,
    /// A cookie-authenticated write came from another origin. A write
    /// with a bearer token needs no `Origin`.
    CrossOrigin,
    /// The request named a host this server does not answer to: an IP
    /// address, `localhost` or a name it was started with.
    UnknownHost,
    NotFound,
    /// A field or header failed the server's checks.
    Invalid,
    /// An edit came without `If-Match`.
    PreconditionRequired,
    /// The request's body is larger than the route takes.
    TooLarge,
    /// The feeder is already adopted.
    AlreadyAdopted,
    /// The feeder was removed from the server: its history stays readable,
    /// and adopting it again brings it back.
    FeederArchived,
    /// Only a phone's token, not a browser session, gets an invite for a
    /// phone: the key is recorded as that token's.
    TokenRequired,
    /// The account or token has an expiry, so its phone holds no key of its
    /// own and reaches the feeders through the server only.
    DirectKeyRefused,
    /// The key is already one of the feeder's clients, or another phone's.
    ClientKeyTaken,
    /// The feeder dropped the invite unanswered: unknown, used or expired.
    InviteExpired,
    /// The server could not reach the feeder.
    Unreachable,
    /// The server has no session with the feeder right now.
    FeederOffline,
    /// The feeder refused the request; `refusal` says why. A refused write is
    /// final for its `Idempotency-Key`: the feeder replays the refusal while
    /// it remembers the id, and a later attempt needs a new key.
    FeederRefused,
    /// No answer arrived in time. The feeder may have run the request:
    /// retry with the same `Idempotency-Key`, which it answers once.
    OutcomeUnknown,
    /// The `Idempotency-Key` already names another request (another
    /// body, feeder, kind of write or user). Nothing was sent.
    KeyReused,
    /// The file is not a release the server keeps; `release` says why.
    ReleaseRefused,
    /// Fetching releases is off (`releases.fetch`); uploads still work.
    FetchDisabled,
    /// The server could not reach the project's releases, or they answered
    /// with something it could not read.
    ReleasesUnreachable,
    Internal,
}

/// Why the feeder refused, as it said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeederRefusal {
    /// The server's key lacks the role.
    Forbidden,
    /// Another change or feed is in progress, or the stores are loading.
    /// Nothing ran. Trying again later is a new request with a new key.
    Busy,
    /// The store moved past the generation the request named.
    Stale,
    /// The firmware does not serve this yet.
    Unavailable,
    Invalid,
    NotWritable,
    /// Every client slot or invite is taken.
    Full,
    NotFound,
    Muted,
    /// The feeder's clock refused the sync.
    Time,
    /// The feeding owner stopped on a fault; the feeder needs a restart.
    Fault,
}

impl From<Refusal> for FeederRefusal {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Forbidden => Self::Forbidden,
            Refusal::Stale { .. } => Self::Stale,
            Refusal::Busy => Self::Busy,
            Refusal::Unavailable => Self::Unavailable,
            Refusal::Invalid => Self::Invalid,
            Refusal::NotWritable => Self::NotWritable,
            Refusal::Full => Self::Full,
            Refusal::NotFound => Self::NotFound,
            Refusal::Muted => Self::Muted,
            Refusal::Time => Self::Time,
            Refusal::Fault => Self::Fault,
            // Update ops and calibration writes are an owner's, never the
            // server's: a server session is refused them as `Forbidden`
            // before these could arrive.
            Refusal::Update(_) | Refusal::Unverified => Self::Forbidden,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Problem {
    /// Always `about:blank`: `code` carries the type.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    pub code: ErrorCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<FeederRefusal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseRefusal>,
}
