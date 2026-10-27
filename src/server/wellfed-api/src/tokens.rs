//! `/tokens`: the bearer tokens a phone or the CLI authenticates with
//! instead of a browser's session cookie.
//!
//! A token is shown once, when it is made, and the server keeps only its
//! hash. It belongs to a user, not a role: a role change applies to it at
//! once, and an account past its expiry or deleted stops it. Each user
//! manages their own; an owner lists and revokes anyone's.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::UserId;

/// The `Authorization` scheme a token travels under:
/// `Authorization: Bearer <secret>`.
pub const BEARER: &str = "Bearer";

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(transparent)]
#[schema(value_type = String, format = Uuid)]
pub struct TokenId(pub Uuid);

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A token to make, for the user asking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewToken {
    /// The device it is for (`Ada's phone`): 1-64 characters.
    pub name: String,
    /// When it stops; it lasts until revoked when absent. The account's
    /// own expiry stops it sooner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<Timestamp>,
}

/// A token as it is listed: never its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Token {
    pub id: TokenId,
    /// Whose it is: it acts as this user, with their role as it is now.
    pub user: UserId,
    /// The device it is for.
    pub name: String,
    pub created: Timestamp,
    /// When it last authenticated a request, to the minute; `None` until
    /// it is first used.
    pub last_used: Option<Timestamp>,
    /// When it stops; `None` lasts until revoked.
    pub expires: Option<Timestamp>,
}

/// A token just made, with its secret: the only time the server shows it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreatedToken {
    #[serde(flatten)]
    pub token: Token,
    /// What the client sends as `Authorization: Bearer <secret>`. The
    /// server keeps only its hash, so a lost secret is revoked and made
    /// again.
    pub secret: String,
}

impl fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedToken")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_secret_is_printed() {
        let created = CreatedToken {
            token: Token {
                id: TokenId(Uuid::nil()),
                user: UserId(Uuid::nil()),
                name: "phone".into(),
                created: Timestamp::UNIX_EPOCH,
                last_used: None,
                expires: None,
            },
            secret: "plt_secretsecret".into(),
        };
        assert!(!format!("{created:?}").contains("secretsecret"));
    }

    #[test]
    fn a_created_token_is_the_token_and_its_secret() {
        let json = serde_json::json!({
            "id": Uuid::nil(),
            "user": Uuid::nil(),
            "name": "phone",
            "created": "1970-01-01T00:00:00Z",
            "last_used": null,
            "expires": null,
            "secret": "plt_00",
        });
        let created: CreatedToken = serde_json::from_value(json).unwrap();
        assert_eq!(created.token.name, "phone");
        assert_eq!(created.secret, "plt_00");
    }
}
