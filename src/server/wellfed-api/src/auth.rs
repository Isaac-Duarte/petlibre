//! Household users, first-owner setup and login.

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::UserId;

/// What a household user may do through the server. Members feed and edit
/// settings; owners also manage users, tokens, feeders and installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Member,
}

impl Role {
    /// The stored spelling.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "owner" => Some(Self::Owner),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

/// Whether a fresh server is waiting for its first owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetupState {
    pub open: bool,
}

/// A name and password: the first owner's at setup, anyone's at login.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Credentials {
    /// 1-64 characters.
    pub name: String,
    /// At least 8 characters.
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
}
