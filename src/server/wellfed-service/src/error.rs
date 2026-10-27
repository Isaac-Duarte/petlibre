//! The service's refusals. [`Error`] holds what every domain shares (a
//! field that failed its checks, a resource that is not there, the
//! feeder's own refusal, the server's own failure) and an enum per domain
//! for the rest, kept beside its domain. Every variant of a domain enum is
//! a refusal the caller can act on, so each gives exactly one API code;
//! the server's own failures go in [`InternalError`], which no caller is
//! told about. [`Error::reply`] gathers them.

use std::fmt;

use wellfed_api::{ErrorCode, FeederRefusal, Settings};
use wellfed_mirror::{AskError, ConnectError, RandomFailed, ZoneError};

use crate::{
    AccountError, BackupError, ClientError, CommandError, ExportError, FeederError, InstallError,
    OwnerDataError,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("{0}")]
    NotFound(Resource),
    /// The feeder refused: final for the request's key, since the feeder
    /// replays its refusal while it remembers the id.
    #[error("the feeder refused: {0:?}")]
    Refused(FeederRefusal),
    /// An edit named a settings generation the feeder has moved past; it
    /// carries the current settings, which the server answers with.
    #[error("the settings moved on to generation {}", .0.generation)]
    Stale(Settings),
    #[error(transparent)]
    Zone(#[from] ZoneError),
    #[error(transparent)]
    Account(#[from] AccountError),
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
    Release(#[from] wellfed_releases::Error),
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error(transparent)]
    Internal(#[from] InternalError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What a [`Error::NotFound`] names. Each reads as the whole refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    User,
    Token,
    /// The household is made at setup, so before it there is none.
    Household,
    Feeder,
    /// A feed the server never journalled nor saw in the feeder's history.
    FeedTrace,
    Pet,
    Weight,
    Food,
    Refill,
    /// A feeder's last install, before any has run.
    Install,
    /// A journal entry under an idempotency key.
    Command,
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "no such user",
            Self::Token => "no such token",
            Self::Household => "the household is made at setup",
            Self::Feeder => "no such feeder",
            Self::FeedTrace => "the server holds no trace of that feed",
            Self::Pet => "no such pet",
            Self::Weight => "no such entry in the pet's weight log",
            Self::Food => "no such food",
            Self::Refill => "no such refill on that feeder",
            Self::Install => "no install has run on that feeder",
            Self::Command => "no command under that key",
        })
    }
}

/// The server's own failures: logged with their sources, and answered
/// `internal` with no detail.
#[derive(Debug, thiserror::Error)]
pub enum InternalError {
    #[error(transparent)]
    Store(#[from] wellfed_store::Error),
    #[error(transparent)]
    Random(#[from] RandomFailed),
    #[error(transparent)]
    Backup(#[from] BackupError),
    #[error("hash a password: {0}")]
    Hash(argon2::password_hash::Error),
    #[error("the password hashing task stopped before it finished")]
    Hashing,
    #[error("a session's expiry is past the latest time the clock can show")]
    SessionExpiry,
    #[error("the mDNS lookup stopped before it answered")]
    Lookup,
    #[error("stored event {0} does not decode")]
    Undecodable(u64),
    #[error("a stored sample does not decode")]
    UndecodableSample,
    #[error("a feeder key does not open with this server's key")]
    Unseal,
    #[error("the feeder sent an answer that does not fit the request")]
    Unexpected,
    #[error("hash a request body: {0}")]
    Body(wellfed_formats::Error),
    #[error("the kept backup is larger than any bank")]
    StockSize,
}

impl From<wellfed_store::Error> for Error {
    fn from(error: wellfed_store::Error) -> Self {
        InternalError::from(error).into()
    }
}

impl From<RandomFailed> for Error {
    fn from(error: RandomFailed) -> Self {
        InternalError::from(error).into()
    }
}

impl From<BackupError> for Error {
    fn from(error: BackupError) -> Self {
        InternalError::from(error).into()
    }
}

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
        let code = match self {
            Self::Invalid(_) => ErrorCode::Invalid,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Refused(_) => ErrorCode::FeederRefused,
            // A real zone no POSIX rule describes is `zone_unsupported`; a
            // name the tzdb does not know is a plain `invalid`.
            Self::Zone(ZoneError::Unknown(_)) => ErrorCode::Invalid,
            Self::Zone(
                ZoneError::NoRule(_) | ZoneError::Unsupported { .. } | ZoneError::Irregular { .. },
            ) => ErrorCode::ZoneUnsupported,
            Self::Account(account) => account.code(),
            Self::Feeder(feeder) => feeder.code(),
            Self::Client(client) => client.code(),
            Self::OwnerData(owner_data) => owner_data.code(),
            Self::Command(command) => command.code(),
            Self::Install(install) => install.code(),
            Self::Release(release) => return crate::operations::releases::reply(release),
            Self::Export(export) => return export.reply(),
            // The server answers a stale edit with the current settings,
            // not a problem.
            Self::Stale(_) | Self::Internal(_) => return Reply::Internal,
        };
        Reply::Code(code)
    }

    /// What the feeder said, for a refusal that is the feeder's.
    pub fn refusal(&self) -> Option<FeederRefusal> {
        match self {
            Self::Refused(refusal) => Some(*refusal),
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
            AskError::NotRunning => Self::NotFound(Resource::Feeder),
            AskError::Offline => CommandError::Offline.into(),
            AskError::Unknown => CommandError::Unknown.into(),
            AskError::Unsendable => InternalError::Unexpected.into(),
        }
    }
}

impl From<ConnectError> for Error {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::Address => Invalid::Address.into(),
            ConnectError::AddressRequired => Invalid::AddressRequired.into(),
            ConnectError::InviteExpired => FeederError::InviteExpired.into(),
            ConnectError::Refused(refusal) => Self::Refused(refusal.into()),
            ConnectError::Unreachable(unreachable) => FeederError::Unreachable(unreachable).into(),
            ConnectError::Random(failed) => failed.into(),
            ConnectError::Lookup => InternalError::Lookup.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn each_kind_of_refusal_is_answered_with_its_code() {
        let busy = Error::Refused(FeederRefusal::Busy);
        let limited = AccountError::Limited {
            retry_after: Duration::from_secs(5),
        };

        assert_eq!(
            Error::NotFound(Resource::Pet).reply(),
            Reply::Code(ErrorCode::NotFound)
        );
        assert_eq!(busy.reply(), Reply::Code(ErrorCode::FeederRefused));
        assert_eq!(busy.refusal(), Some(FeederRefusal::Busy));
        assert_eq!(
            Error::from(limited).reply(),
            Reply::Code(ErrorCode::LoginLimited)
        );
        let internal = Error::from(wellfed_store::Error::Corrupt("feeder"));
        assert_eq!(internal.reply(), Reply::Internal);
        assert_eq!(internal.refusal(), None);
    }

    /// A real zone no POSIX rule describes is `zone_unsupported`, named in
    /// the detail; a name the tzdb does not know is a plain `invalid`.
    #[test]
    fn a_zone_is_refused_by_kind() {
        let irregular = Error::Zone(ZoneError::Irregular {
            name: "Asia/Gaza".into(),
            rule: "EET-2EEST".into(),
            at: jiff::Timestamp::UNIX_EPOCH,
        });
        let unknown = Error::Zone(ZoneError::Unknown("Mars/Olympus".into()));

        assert_eq!(irregular.reply(), Reply::Code(ErrorCode::ZoneUnsupported));
        assert!(irregular.to_string().contains("Asia/Gaza"));
        assert_eq!(unknown.reply(), Reply::Code(ErrorCode::Invalid));
    }
}
