//! Household users, first-owner setup and login.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::UserId;

/// What a household user may do through the server. Owners do everything;
/// members also feed and change settings and the lock; viewers read status
/// and history and write nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Member,
    Viewer,
}

impl Role {
    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Owner, Self::Member, Self::Viewer]
            .into_iter()
            .find(|role| role.code() == code)
    }
}

/// Whether a fresh server is waiting for its first owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetupState {
    pub open: bool,
    /// The server host's IANA zone, when it has one: the setup page offers
    /// it as the household's zone.
    pub zone: Option<String>,
}

/// The first owner and the household's zone.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Setup {
    /// 1-64 characters.
    pub name: String,
    /// At least 10 characters.
    pub password: String,
    /// The household's IANA zone (`Europe/Berlin`). Every feeder keeps its
    /// POSIX rule, so a zone the rule cannot describe is refused
    /// `zone_unsupported`.
    pub zone: String,
}

impl fmt::Debug for Setup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setup")
            .field("name", &self.name)
            .field("zone", &self.zone)
            .finish_non_exhaustive()
    }
}

/// A name and password, to log in.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Credentials {
    pub name: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct User {
    pub id: UserId,
    pub name: String,
    pub role: Role,
    /// When the account stops: past it, its sessions no longer work and it
    /// cannot log in, until an owner extends or deletes it. Owners never
    /// expire.
    pub expires: Option<Timestamp>,
}

/// A user changing their own password: `PATCH /session`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PasswordChange {
    /// The password the user logs in with now.
    pub current: String,
    /// At least 10 characters.
    pub password: String,
}

impl fmt::Debug for PasswordChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordChange").finish_non_exhaustive()
    }
}
