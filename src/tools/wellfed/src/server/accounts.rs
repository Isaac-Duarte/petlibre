//! What only the server has of its people: the household's accounts and
//! the tokens.
//!
//! A token cannot add an account or set a password (only a login can), so
//! these verbs list, change a role or an expiry, and delete; adding an
//! account is the server's web page's.

use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use jiff::Timestamp;
use wellfed_server_client::Client;
use wellfed_server_client::api::{Role, TokenId, User, UserId, UserPatch};
use uuid::Uuid;

use super::{Output, Settled, text};

#[derive(Subcommand)]
pub(crate) enum Users {
    /// Every account, by name.
    List,
    /// Change an account's role or expiry.
    Set {
        /// The account's name or id.
        user: String,
        #[arg(long, value_enum)]
        role: Option<AccountRole>,
        /// When the account stops working (RFC 3339, like
        /// `2026-12-31T23:00:00Z`), or `never`.
        #[arg(long, value_parser = parse_expiry)]
        expires: Option<Expiry>,
    },
    /// Delete an account and end its sessions and tokens.
    Delete {
        /// The account's name or id.
        user: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum Tokens {
    /// Your tokens, or everyone's for an owner. No secret is listed.
    List,
    /// Revoke a token by its id: your own, or anyone's for an owner.
    Revoke { id: Uuid },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum AccountRole {
    Owner,
    Member,
    Viewer,
}

impl AccountRole {
    const fn api(self) -> Role {
        match self {
            Self::Owner => Role::Owner,
            Self::Member => Role::Member,
            Self::Viewer => Role::Viewer,
        }
    }
}

/// An account's expiry as `--expires` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expiry {
    Never,
    At(Timestamp),
}

/// An `--expires` value that is neither a time nor `never`.
#[derive(Debug, thiserror::Error)]
#[error("{text:?} is neither an RFC 3339 time nor `never`")]
pub(crate) struct NotExpiry {
    text: String,
}

fn parse_expiry(text: &str) -> Result<Expiry, NotExpiry> {
    if text == "never" {
        return Ok(Expiry::Never);
    }
    text.parse().map(Expiry::At).map_err(|_| NotExpiry {
        text: text.to_owned(),
    })
}

pub(super) async fn users(client: &Client, users: Users, out: Output) -> Result<Settled> {
    match users {
        Users::List => {
            let users = client.users().await?;
            out.show(&users, |users| text::lines(users, text::user))?;
        }
        Users::Set {
            user,
            role,
            expires,
        } => {
            if role.is_none() && expires.is_none() {
                bail!("pass --role or --expires");
            }
            let id = account(client, &user).await?;
            let patch = UserPatch {
                role: role.map(AccountRole::api),
                expires: expires.map(|expiry| match expiry {
                    Expiry::Never => None,
                    Expiry::At(at) => Some(at),
                }),
                password: None,
            };
            let changed = client.update_user(id, &patch).await?;
            out.show(&changed, text::user)?;
        }
        Users::Delete { user } => {
            let id = account(client, &user).await?;
            client.delete_user(id).await?;
            out.text(format!("Deleted {user}."));
        }
    }
    Ok(Settled::Known)
}

/// The account named `which`, by name or id.
async fn account(client: &Client, which: &str) -> Result<UserId> {
    let users = client.users().await?;
    let matching: Vec<&User> = users
        .iter()
        .filter(|user| user.name == which || user.id.to_string() == which.to_lowercase())
        .collect();
    let [user] = matching.as_slice() else {
        bail!(
            "{} accounts are named or numbered {which:?}",
            matching.len()
        );
    };
    Ok(user.id)
}

pub(super) async fn tokens(client: &Client, tokens: Tokens, out: Output) -> Result<Settled> {
    match tokens {
        Tokens::List => {
            let tokens = client.tokens().await?;
            out.show(&tokens, |tokens| text::lines(tokens, text::token))?;
        }
        Tokens::Revoke { id } => {
            client.revoke_token(TokenId(id)).await?;
            out.text(format!("Revoked token {id}."));
        }
    }
    Ok(Settled::Known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expiry_is_a_time_or_never() {
        assert_eq!(parse_expiry("never").ok(), Some(Expiry::Never));
        assert_eq!(
            parse_expiry("2026-12-31T23:00:00Z").ok(),
            Some(Expiry::At(Timestamp::from_second(1_798_758_000).unwrap()))
        );
        for bad in ["", "tomorrow", "2026-12-31"] {
            assert!(parse_expiry(bad).is_err(), "{bad}");
        }
    }
}
