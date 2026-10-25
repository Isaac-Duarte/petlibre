//! Errors as RFC 9457 `application/problem+json`, each with a typed code.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Every way a request fails. Clients branch on this, never on the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The first owner exists; setup is closed.
    SetupClosed,
    /// No valid session.
    Unauthenticated,
    BadCredentials,
    /// This user's role may not do that.
    Forbidden,
    /// A cookie-authenticated write came from another origin.
    CrossOrigin,
    /// The request named a host this server does not answer to: an IP
    /// address, `localhost` or a name it was started with.
    UnknownHost,
    NotFound,
    /// A field or header failed the server's checks.
    Invalid,
    /// An edit came without `If-Match`.
    PreconditionRequired,
    /// The feeder is already adopted.
    AlreadyAdopted,
    /// The feeder dropped the invite unanswered: unknown, used or expired.
    InviteExpired,
    /// The server could not reach the feeder.
    Unreachable,
    /// The server has no session with the feeder right now.
    FeederOffline,
    /// The feeder refused the request; `refusal` says why.
    FeederRefused,
    /// No answer arrived in time. The feeder may have run the request:
    /// retry with the same `Idempotency-Key`, which it answers once.
    OutcomeUnknown,
    Internal,
}

/// Why the feeder refused, as it said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeederRefusal {
    /// The server's key lacks the role.
    Forbidden,
    /// Another change is being written, or the stores are loading.
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
}
