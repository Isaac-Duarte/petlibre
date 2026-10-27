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
    Token(#[from] TokenError),
    #[error(transparent)]
    Household(#[from] HouseholdError),
    #[error(transparent)]
    Feeder(#[from] FeederError),
    #[error(transparent)]
    OwnerData(#[from] OwnerDataError),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Stock(#[from] StockError),
    #[error(transparent)]
    Random(#[from] RandomFailed),
    #[error(transparent)]
    Release(#[from] wellfed_releases::Error),
    #[error(transparent)]
    Backup(#[from] crate::BackupError),
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
    #[error("a token's name is 1-64 characters")]
    TokenName,
    #[error("a token's expiry is in the future")]
    TokenExpiry,
    #[error("a page holds 1-1000 items")]
    PageLimit,
    #[error("not a cursor this server gave")]
    PageCursor,
    #[error("a pet's name is 1-64 characters")]
    PetName,
    #[error("a daily target is 1-10000 kcal")]
    DailyKcal,
    #[error("a weight is 1-200000 g")]
    Weight,
    #[error("a weight is read now or in the past")]
    WeightTime,
    #[error("a food's brand and name are 1-64 characters each")]
    FoodName,
    #[error("a food's energy is 1-10000 kcal per kg")]
    Kcal,
    #[error("a bag is 1-100000 g")]
    BagSize,
    #[error("a cup is 1-1000 g")]
    GramsPerCup,
    #[error("a refill is 1-100000 g")]
    RefillGrams,
    #[error("a sample span's `from` is before its `to`")]
    SampleSpan,
}

/// Setup, login, sessions and the accounts owners manage.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("setup is closed: an account exists")]
    SetupClosed,
    #[error("wrong name or password")]
    BadCredentials,
    #[error("too many failed logins: try again in {} s", retry_after.as_secs())]
    /// `retry_after` is whole seconds.
    Limited { retry_after: std::time::Duration },
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

/// Bearer tokens.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("no such token")]
    NotFound,
    #[error("only a login makes a token: a token cannot make another")]
    SessionRequired,
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
    #[error("a stored sample does not decode")]
    UndecodableSample,
    #[error("a feeder key does not open with this server's key")]
    Unseal,
    #[error("the server holds no trace of that feed")]
    NoTrace,
    #[error("the feeder did not answer the log request in time")]
    LogUnanswered,
}

/// Pets, foods and refills: the owner data the feeders do not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OwnerDataError {
    #[error("no such pet")]
    PetNotFound,
    #[error("no such entry in the pet's weight log")]
    WeightNotFound,
    #[error("that feeder does not serve the pet")]
    NotAssigned,
    #[error("no such food")]
    FoodNotFound,
    #[error("a feeder's food history names the food: past feeds' calories use it")]
    FoodInUse,
    #[error("no such refill on that feeder")]
    RefillNotFound,
    #[error("the feeder has no food set, so a full bag weighs nothing known: set its food first")]
    NoCurrentFood,
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

/// Installs, one at a time per server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("another install is running: one runs at a time")]
    Running,
    #[error("no install has run on that feeder")]
    None,
}

/// The feeder's stock backup and its restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StockError {
    #[error("the restore does not offer the kept backup: {0}")]
    Mismatch(wellfed_releases::RestoreError),
    #[error("the kept backup is larger than any bank")]
    Size,
    #[error(
        "the feeder runs from bank 0 with stock in bank 1, and no verified backup of it is kept: \
         read it from the feeder or upload the installer's copy before installing"
    )]
    BackupNeeded,
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
