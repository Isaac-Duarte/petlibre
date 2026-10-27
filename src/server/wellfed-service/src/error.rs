//! The service's refusals: an enum per domain, wrapped in [`Error`], so a
//! handler's mapping names each domain's cases and a new domain adds an
//! enum rather than growing one list.

use wellfed_api::{FeederRefusal, Settings};
use wellfed_mirror::{AskError, ConnectError, InviteText, RandomFailed, Unreachable, ZoneError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] wellfed_store::Error),
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    Account(#[from] AccountError),
    #[error(transparent)]
    Household(#[from] HouseholdError),
    #[error(transparent)]
    Feeder(#[from] FeederError),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error(transparent)]
    Random(#[from] RandomFailed),
    #[error(transparent)]
    Release(#[from] wellfed_releases::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A field that failed the server's checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("a user name is 1-64 characters")]
    UserName,
    #[error("a password is at least 10 characters")]
    Password,
    #[error("an owner's account does not expire: remove the expiry, or pick another role")]
    OwnerExpires,
    #[error("a household name is 1-64 characters")]
    HouseholdName,
    #[error("a feeder name is 1-64 characters")]
    FeederName,
    #[error("the address is not an IP or IP:port")]
    Address,
    #[error("give the feeder's address: discovery is off")]
    AddressRequired,
    #[error("the volume is 0-100")]
    Volume,
    #[error("a page holds 1-500 events")]
    Limit,
    #[error("the server's label is 1-16 bytes with no NUL")]
    Label,
    #[error("an event cursor is at most 2^63 - 1")]
    Cursor,
    #[error("a page holds 1-1000 commands")]
    CommandLimit,
    #[error("not a cursor this server gave")]
    CommandCursor,
}

/// Setup, login, sessions and the accounts owners manage.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("setup is closed: an account exists")]
    SetupClosed,
    #[error("wrong name or password")]
    BadCredentials,
    #[error("the account is past its expiry: an owner can extend it")]
    Disabled,
    #[error("no such user")]
    NotFound,
    #[error("that name is taken")]
    NameTaken,
    #[error("the household would have no owner left")]
    LastOwner,
    #[error("{0} is not an owner")]
    NotAnOwner(String),
    #[error("the household has no owner")]
    NoOwner,
    #[error("the household has several owners: name one")]
    OwnerAmbiguous,
    #[error("hash a password: {0}")]
    Hash(argon2::password_hash::Error),
    #[error("the password hashing task stopped before it finished")]
    Hashing,
    #[error("a session's expiry is past the latest time the clock can show")]
    Expiry,
}

/// The household's settings.
#[derive(Debug, thiserror::Error)]
pub enum HouseholdError {
    #[error("the household is made at setup")]
    NotSetUp,
    #[error(transparent)]
    Zone(ZoneError),
}

/// Adopting feeders and reading what the server holds of them.
#[derive(Debug, thiserror::Error)]
pub enum FeederError {
    #[error("no such feeder")]
    NotFound,
    #[error("that feeder is already adopted")]
    AlreadyAdopted,
    #[error(transparent)]
    Invite(#[from] InviteText),
    #[error("the feeder dropped the invite: unknown, used or expired")]
    InviteExpired,
    #[error(transparent)]
    Unreachable(#[from] Unreachable),
    #[error("the feeder refused: {0:?}")]
    Refused(FeederRefusal),
    #[error("the mDNS lookup stopped before it answered")]
    Lookup,
    #[error("stored event {0} does not decode")]
    Undecodable(u64),
    #[error("a feeder key does not open with this server's key")]
    Unseal,
}

/// A command sent to a feeder, and its answer.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("the server has no session with the feeder")]
    Offline,
    #[error("no answer in time: retry with the same key")]
    Unknown,
    #[error("the feeder refused: {0:?}")]
    Refused(FeederRefusal),
    #[error("the settings moved on to generation {}", .0.generation)]
    Stale(Settings),
    #[error("the feeder sent an answer that does not fit the request")]
    Unexpected,
    #[error("the Idempotency-Key already names another request; nothing was sent")]
    KeyReused,
    #[error("no command under that key")]
    NoEntry,
    #[error("hash a request body: {0}")]
    Body(wellfed_formats::Error),
}

/// A mirror refusal in the service's domains. Only `Unknown` may have
/// reached the feeder; the rest were never sent.
impl From<AskError> for Error {
    fn from(error: AskError) -> Self {
        match error {
            AskError::NotRunning => FeederError::NotFound.into(),
            AskError::Offline => CommandError::Offline.into(),
            AskError::Unknown => CommandError::Unknown.into(),
            AskError::Unsendable => CommandError::Unexpected.into(),
        }
    }
}

impl From<ConnectError> for Error {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::Address => Invalid::Address.into(),
            ConnectError::AddressRequired => Invalid::AddressRequired.into(),
            ConnectError::InviteExpired => FeederError::InviteExpired.into(),
            ConnectError::Refused(refusal) => FeederError::Refused(refusal.into()).into(),
            ConnectError::Unreachable(unreachable) => FeederError::Unreachable(unreachable).into(),
            ConnectError::Random(failed) => failed.into(),
            ConnectError::Lookup => FeederError::Lookup.into(),
        }
    }
}
