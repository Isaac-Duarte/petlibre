//! The service's refusals: an enum per domain, kept beside its domain and
//! wrapped in [`Error`], so a new domain adds an enum rather than growing
//! one list. Each domain also says how its refusals are answered
//! ([`Reply`]), so the code a client sees is decided where the refusal is.

use wellfed_api::{ErrorCode, FeederRefusal};
use wellfed_mirror::{AskError, ConnectError, RandomFailed};

use crate::{
    AccountError, BackupError, ClientError, CommandError, ExportError, FeederError, HouseholdError,
    InstallError, OwnerDataError, StockError, TokenError,
};

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
    Client(#[from] ClientError),
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
    Backup(#[from] BackupError),
    #[error(transparent)]
    Export(#[from] ExportError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How a refusal is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The caller's to act on: the code, with the error's message as the
    /// detail.
    Code(ErrorCode),
    /// As [`Reply::Code`], but its cause (a status, a TLS failure, the
    /// database's own words) is only in the error's sources, which the
    /// server logs as a warning.
    Warn(ErrorCode),
    /// The server's own failure: logged with its sources, and answered
    /// `internal` with no detail.
    Internal,
}

impl Error {
    pub fn reply(&self) -> Reply {
        match self {
            Self::Invalid(_) => Reply::Code(ErrorCode::Invalid),
            Self::Account(account) => account.reply(),
            Self::Token(token) => Reply::Code(token.code()),
            Self::Household(household) => Reply::Code(household.code()),
            Self::Feeder(feeder) => feeder.reply(),
            Self::Client(client) => Reply::Code(client.code()),
            Self::OwnerData(owner_data) => Reply::Code(owner_data.code()),
            Self::Command(command) => command.reply(),
            Self::Install(install) => Reply::Code(install.code()),
            Self::Stock(stock) => stock.reply(),
            Self::Release(release) => crate::operations::releases::reply(release),
            Self::Export(export) => export.reply(),
            Self::Store(_) | Self::Random(_) | Self::Backup(_) => Reply::Internal,
        }
    }

    /// What the feeder said when it refused, for a refusal that is the
    /// feeder's.
    pub fn refusal(&self) -> Option<FeederRefusal> {
        match self {
            Self::Feeder(FeederError::Refused(refusal))
            | Self::Command(CommandError::Refused(refusal)) => Some(*refusal),
            _ => None,
        }
    }
}

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
    #[error("an SSID is 1-32 bytes")]
    Ssid,
    #[error("a WPA2 passphrase is 8-63 printable ASCII characters")]
    Psk,
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
