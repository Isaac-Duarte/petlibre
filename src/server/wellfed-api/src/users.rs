//! `/users`: the household's accounts, managed by owners.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::Role;

/// A new account.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewUser {
    /// 1-64 characters, unique.
    pub name: String,
    /// At least 10 characters: a temporary one the owner passes on.
    pub password: String,
    pub role: Role,
    /// When the account stops (a pet sitter's); never when absent. An
    /// owner's account cannot expire.
    #[serde(default)]
    pub expires: Option<Timestamp>,
}

impl fmt::Debug for NewUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewUser")
            .field("name", &self.name)
            .field("role", &self.role)
            .field("expires", &self.expires)
            .finish_non_exhaustive()
    }
}

/// A change to an account; an absent field is left as it is.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UserPatch {
    /// Demoting the last owner is refused `last_owner`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// A time sets or moves the expiry, `null` removes it. Extending a
    /// disabled account enables it again.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<Timestamp>, nullable)]
    pub expires: Option<Option<Timestamp>>,
    /// A temporary password (at least 10 characters). Setting another
    /// user's ends their sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl fmt::Debug for UserPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserPatch")
            .field("role", &self.role)
            .field("expires", &self.expires)
            .field("password", &self.password.as_ref().map(|_| ".."))
            .finish()
    }
}

/// A field that is present is `Some`, even when it is `null`: absent
/// leaves a value alone, `null` clears it.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_expiry_is_kept_and_null_clears_it() {
        let parse = |json| serde_json::from_str::<UserPatch>(json).unwrap().expires;

        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"expires":null}"#), Some(None));
        assert_eq!(
            parse(r#"{"expires":"2026-11-01T00:00:00Z"}"#),
            Some(Some("2026-11-01T00:00:00Z".parse().unwrap()))
        );
        let cleared = UserPatch {
            expires: Some(None),
            ..UserPatch::default()
        };
        assert_eq!(
            serde_json::to_string(&cleared).unwrap(),
            r#"{"expires":null}"#
        );
    }

    #[test]
    fn no_password_is_printed() {
        let patch = UserPatch {
            password: Some("hunter2hunter2".into()),
            ..UserPatch::default()
        };
        assert!(!format!("{patch:?}").contains("hunter2"));
    }
}
