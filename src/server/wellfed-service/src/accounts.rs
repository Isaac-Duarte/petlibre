//! Accounts: the household's owners, members and viewers, how each signs
//! in (a login's session, or a bearer token for a phone or the CLI), the
//! failed-login limits, what each role may do, and the audit log of what
//! they did.

mod audit;
mod limits;
mod permissions;
pub(crate) mod sessions;
pub(crate) mod tokens;
mod users;

use std::sync::Arc;
use std::time::Duration;

use wellfed_api::ErrorCode;

pub use permissions::{Permission, can};
pub use sessions::SessionToken;
pub use tokens::{BearerToken, Caller, TokenError};
pub use users::reset_owner;

use crate::Reply;

/// What the accounts keep between requests.
#[derive(Clone, Default)]
pub(crate) struct State {
    /// Failed logins per name and per address.
    pub(crate) limits: Arc<limits::Limits>,
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
    Limited { retry_after: Duration },
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

impl AccountError {
    pub(crate) fn reply(&self) -> Reply {
        let code = match self {
            Self::SetupClosed => ErrorCode::SetupClosed,
            Self::BadCredentials => ErrorCode::BadCredentials,
            Self::Limited { .. } => ErrorCode::LoginLimited,
            Self::Disabled => ErrorCode::AccountDisabled,
            Self::NotFound => ErrorCode::NotFound,
            Self::NameTaken => ErrorCode::NameTaken,
            Self::LastOwner => ErrorCode::LastOwner,
            // Only `reset-owner`, on the host, picks an owner by name.
            Self::NotAnOwner(_) | Self::NoOwner | Self::OwnerAmbiguous => ErrorCode::Invalid,
            Self::Hash(_) | Self::Hashing | Self::Expiry => return Reply::Internal,
        };
        Reply::Code(code)
    }
}
