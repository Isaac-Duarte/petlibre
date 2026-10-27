//! The mirror's refusals, one enum per kind of call.

use wellfed_model::Refusal;

use crate::RandomFailed;

/// Why a command got no answer. Only [`AskError::Unknown`] may have reached
/// the feeder: every other refusal was never sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AskError {
    #[error("no such feeder")]
    NotRunning,
    #[error("the server has no session with the feeder")]
    Offline,
    #[error("no answer in time: retry with the same key")]
    Unknown,
    /// Refused before anything was sent (a reserved id, or an op that does
    /// not encode).
    #[error("the request could not be sent")]
    Unsendable,
}

/// The feeder could not be reached.
#[derive(Debug, thiserror::Error)]
#[error("reach the feeder: {0}")]
pub struct Unreachable(pub(crate) wellfed_connect::Error);

/// Why a feeder's debug log could not be followed. Nothing it asks for
/// changes the feeder, so none of these leaves anything uncertain.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("no such feeder")]
    NotRunning,
    #[error("the server has no session with the feeder")]
    Offline,
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("the feeder refused: {0:?}")]
    Refused(Refusal),
    #[error("the feeder did not answer the log request in time")]
    Unanswered,
    #[error("the feeder answered the log request with something else")]
    Unexpected,
}

/// Why a feeder could not be located, or an invite redeemed.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("the address is not an IP or IP:port")]
    Address,
    #[error("give the feeder's address: discovery is off")]
    AddressRequired,
    #[error("the feeder dropped the invite: unknown, used or expired")]
    InviteExpired,
    #[error("the feeder refused: {0:?}")]
    Refused(Refusal),
    #[error(transparent)]
    Unreachable(#[from] Unreachable),
    #[error(transparent)]
    Random(#[from] RandomFailed),
    #[error("the mDNS lookup stopped before it answered")]
    Lookup,
}
