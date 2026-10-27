//! Bearer tokens: how a phone or the CLI authenticates without a browser
//! session.
//!
//! A token's secret is `plt_` and 64 lowercase hex digits: 32 random bytes,
//! which is beyond guessing, so the store keeps a plain SHA-256 of them
//! rather than a slow password hash, and a copied database holds no live
//! token. The secret is shown once, when it is made.
//!
//! A token is its user's and carries no role of its own: each use reads
//! the user as they are now, so a role change applies at once, deleting
//! the user deletes their tokens, and an account past its expiry has its
//! tokens refused, though kept, until an owner extends it. A token lasts
//! until it is revoked, or until its own expiry if it was given one.
//!
//! Only a login makes a token, sets a password or adds an account, so
//! revoking a leaked token is enough: it cannot have made other tokens,
//! nor a password or an account to log in with and make them.

use wellfed_api::{AuditAction, CreatedToken, NewToken, RevokeReason, Token, TokenId, User};
use wellfed_formats::hex;
use wellfed_store::Whose;
use sha2::{Digest, Sha256};

use crate::sessions::{TOUCH, disabled};
use crate::{AccountError, Invalid, Permission, Result, Service, TokenError, can};

/// What every secret starts with, so it is recognisable in a config file
/// or a leak scan, and never mistaken for a session's.
const PREFIX: &str = "plt_";

/// The longest device name a token takes.
const NAME_MAX: usize = 64;

/// Who is asking: the user, and the token they came through (`None` for
/// a browser session), which the command journal records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub user: User,
    pub token: Option<TokenId>,
}

/// A browser session's caller.
impl From<User> for Caller {
    fn from(user: User) -> Self {
        Self { user, token: None }
    }
}

/// A bearer token's secret, as a client presents it.
#[derive(Clone, PartialEq, Eq)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(text: String) -> Self {
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The stored form; `None` for a secret this server could not have
    /// made.
    fn hash(&self) -> Option<[u8; 32]> {
        let digits = self.0.strip_prefix(PREFIX)?;
        // Lowercase only: the one spelling this server writes.
        if !hex::is_lower_hex(digits, 64) {
            return None;
        }
        let bytes = hex::decode::<32>(digits)?;
        Some(Sha256::digest(bytes).into())
    }
}

impl std::fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BearerToken").finish_non_exhaustive()
    }
}

/// Refuse a token's caller [`TokenError::SessionRequired`]: what a leaked
/// token could turn into a login of its own takes a login.
pub(crate) fn session_only(caller: &Caller) -> Result<()> {
    if caller.token.is_some() {
        return Err(TokenError::SessionRequired.into());
    }
    Ok(())
}

impl Service {
    /// The caller a bearer token stands for, while it lasts, and its use
    /// recorded. An expired or unknown token is `None`; one whose account is
    /// past its expiry is refused [`AccountError::Disabled`].
    pub async fn authenticate_token(&self, token: &BearerToken) -> Result<Option<Caller>> {
        let Some(hash) = token.hash() else {
            return Ok(None);
        };
        let Some(found) = self.shared.store.token_use(&hash).await? else {
            return Ok(None);
        };

        let now = self.shared.clock.now();
        if found.token.expires.is_some_and(|expires| expires <= now) {
            return Ok(None);
        }
        if disabled(&found.user, now) {
            return Err(AccountError::Disabled.into());
        }
        let stale = found
            .token
            .last_used
            .is_none_or(|used| used.checked_add(TOUCH).is_ok_and(|due| due <= now));
        if stale {
            self.shared.store.touch_token(found.token.id, now).await?;
        }
        Ok(Some(Caller {
            user: found.user,
            token: Some(found.token.id),
        }))
    }

    /// Make a token for the caller, who logged in: a token cannot make
    /// another. The secret is in the answer and nowhere else.
    pub async fn create_token(&self, caller: &Caller, new: NewToken) -> Result<CreatedToken> {
        session_only(caller)?;
        let name = new.name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX {
            return Err(Invalid::TokenName.into());
        }
        let now = self.shared.clock.now();
        if new.expires.is_some_and(|expires| expires <= now) {
            return Err(Invalid::TokenExpiry.into());
        }

        let secret = self.shared.random::<32>()?;
        let hash: [u8; 32] = Sha256::digest(secret).into();
        let token = Token {
            id: TokenId(self.shared.new_id()?),
            user: caller.user.id,
            name: name.to_owned(),
            created: now,
            last_used: None,
            expires: new.expires,
        };
        self.shared
            .store
            .create_token(wellfed_store::NewToken {
                id: token.id,
                hash: &hash,
                user: token.user,
                name: &token.name,
                created: token.created,
                expires: token.expires,
            })
            .await?;

        let action = AuditAction::TokenCreated {
            token: token.id,
            account: token.user,
            token_name: token.name.clone(),
        };
        self.record(caller, action).await;
        Ok(CreatedToken {
            token,
            secret: format!("{PREFIX}{}", hex::encode(&secret)),
        })
    }

    /// The user's own tokens, or everyone's for an owner, oldest first.
    pub async fn tokens(&self, user: &User) -> Result<Vec<Token>> {
        let whose = (!can(user.role, Permission::Users)).then_some(user.id);
        Ok(self.shared.store.tokens(whose).await?)
    }

    /// Revoke a token: the caller's own, or anyone's for an owner.
    /// Another user's is not found, so a member learns nothing of it. The
    /// phone key it was invited for is revoked on its feeder.
    pub async fn revoke_token(&self, caller: &Caller, id: TokenId) -> Result<()> {
        let user = &caller.user;
        let token = self.shared.store.token(id).await?;
        let Some(token) =
            token.filter(|token| token.user == user.id || can(user.role, Permission::Users))
        else {
            return Err(TokenError::NotFound.into());
        };
        if !self.shared.store.delete_token(id).await? {
            return Err(TokenError::NotFound.into());
        }

        let action = AuditAction::TokenRevoked {
            token: id,
            account: token.user,
            token_name: token.name,
        };
        self.record(caller, action).await;
        self.revoke_phones(Whose::Token(id), RevokeReason::TokenRevoked)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_secret_this_server_writes_hashes() {
        let digits = "ab".repeat(32);
        assert!(BearerToken::new(format!("plt_{digits}")).hash().is_some());

        for odd in [
            digits.clone(),
            format!("plt_{}", digits.to_uppercase()),
            format!("plt_{}", &digits[2..]),
            format!("plt_{digits}00"),
            format!("pat_{digits}"),
            String::new(),
        ] {
            assert!(BearerToken::new(odd.clone()).hash().is_none(), "{odd}");
        }
    }

    #[test]
    fn no_secret_is_printed() {
        let token = BearerToken::new("plt_secretsecret".into());
        assert!(!format!("{token:?}").contains("secretsecret"));
    }
}
