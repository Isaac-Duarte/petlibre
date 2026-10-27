//! `/audit`: who did what through the server, and when.
//!
//! The log is the command journal (every write sent to a feeder, with its
//! state and outcome as they stand now) interleaved with the account
//! changes the server made: logins, failed logins, users, passwords,
//! tokens and the household. An account change is recorded once it is
//! made, so a refused one leaves no entry; a failed login is the
//! exception, recorded against the account whose name was tried. Like the
//! journal, the log is kept forever, and an entry keeps the name its user
//! had, so it outlives the account.

use std::net::IpAddr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Command, Household, Role, TokenId, UserId};

/// One thing that happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AuditEntry {
    /// When it happened; for a command, when it was recorded, before it
    /// was sent.
    pub at: Timestamp,
    /// Who did it. `None` only for a password the host's `reset-owner`
    /// set.
    pub user: Option<UserId>,
    /// Their name when it happened, kept after the account is deleted.
    pub user_name: Option<String>,
    /// The token it came through; `None` for a browser session.
    pub token: Option<TokenId>,
    pub action: AuditAction,
}

/// What happened. A change names the account it changed (`account`, with
/// its `name`) and its new values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditAction {
    /// A write sent to a feeder, as the journal holds it now: its state
    /// and outcome follow it until it settles.
    Command {
        command: Command,
    },
    /// The first owner and the household were made.
    Setup,
    /// A login from `from`, the client's address.
    Login {
        #[schema(value_type = String)]
        from: IpAddr,
    },
    /// A login with this account's name that failed. A login refused by
    /// the limits before its password was checked is not recorded.
    LoginFailed {
        #[schema(value_type = String)]
        from: IpAddr,
        reason: LoginFailure,
    },
    UserCreated {
        account: UserId,
        name: String,
        role: Role,
        expires: Option<Timestamp>,
    },
    /// A role or expiry changed.
    UserChanged {
        account: UserId,
        name: String,
        role: Role,
        expires: Option<Timestamp>,
    },
    /// A password set: the user's own, or an owner's temporary one for
    /// them, or the host's `reset-owner`.
    PasswordSet {
        account: UserId,
        name: String,
    },
    UserDeleted {
        account: UserId,
        name: String,
    },
    /// A token made for `account` (always the caller's own), named
    /// `token_name` for its device.
    TokenCreated {
        token: TokenId,
        account: UserId,
        token_name: String,
    },
    TokenRevoked {
        token: TokenId,
        account: UserId,
        token_name: String,
    },
    /// The household's settings, as they now stand.
    HouseholdChanged {
        household: Household,
    },
}

/// Why a login failed after its password was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginFailure {
    /// Not the account's password.
    BadPassword,
    /// The right password, on an account past its expiry.
    Disabled,
}

/// A page of the log, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AuditPage {
    pub items: Vec<AuditEntry>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}
