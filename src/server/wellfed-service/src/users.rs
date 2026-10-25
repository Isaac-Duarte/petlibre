//! Household users: first-owner setup, login and browser sessions.
//!
//! Passwords are argon2id hashes, computed off the async threads and at
//! most [`HASHING`] at a time: each takes about 19 MiB, and logins need no
//! account, so a flood of them must queue rather than exhaust memory. A
//! session token is 32 random bytes the browser holds; the store keeps its
//! SHA-256, so a copied database holds no live session.

use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use wellfed_api::{Credentials, Role, User, UserId};
use wellfed_session::hex::{self, Lower};
use wellfed_store::NewUser;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Error, Invalid, Result, Service};

/// Password hashes computed at once.
const HASHING: usize = 2;
static HASHERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(HASHING);

/// A browser session's secret, as its cookie carries it.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    pub fn new(text: String) -> Self {
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The stored form; `None` for a token this server could not have made.
    fn hash(&self) -> Option<[u8; 32]> {
        let bytes = hex::decode_array::<32>(self.0.as_bytes())?;
        Some(Sha256::digest(bytes).into())
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionToken").finish_non_exhaustive()
    }
}

impl Service {
    /// Whether a fresh server is waiting for its first owner.
    pub async fn setup_open(&self) -> Result<bool> {
        Ok(!self.shared.store.users_exist().await?)
    }

    /// Create the first owner and log them in. Refused once any user
    /// exists, even when two setups race.
    pub async fn setup(&self, credentials: Credentials) -> Result<(User, SessionToken)> {
        let name = user_name(&credentials.name)?;
        if credentials.password.chars().count() < 8 {
            return Err(Invalid::Password.into());
        }
        if !self.setup_open().await? {
            return Err(Error::SetupClosed);
        }

        let salt = self.shared.random::<16>()?;
        let hash = blocking(move || hash_password(&credentials.password, &salt)).await?;
        let user = User {
            id: UserId(Uuid::now_v7()),
            name: name.to_owned(),
            role: Role::Owner,
        };
        let created = self
            .shared
            .store
            .create_first_owner(NewUser {
                id: user.id,
                name: &user.name,
                password_hash: &hash,
                role: user.role,
                created: self.shared.clock.now(),
            })
            .await?;
        if !created {
            return Err(Error::SetupClosed);
        }
        let token = self.open_session(user.id).await?;
        Ok((user, token))
    }

    /// Check a name and password and open a session. An unknown name costs
    /// the same hash as a wrong password, so timing does not reveal names.
    pub async fn login(&self, credentials: Credentials) -> Result<(User, SessionToken)> {
        let login = self.shared.store.login(credentials.name.trim()).await?;
        let (user, hash) = match login {
            Some(login) => (Some(login.user), login.password_hash),
            None => (None, decoy()?.to_owned()),
        };

        let matches = blocking(move || Ok(verify_password(&credentials.password, &hash))).await?;
        let (Some(user), true) = (user, matches) else {
            return Err(Error::BadCredentials);
        };
        let token = self.open_session(user.id).await?;
        Ok((user, token))
    }

    pub async fn logout(&self, token: &SessionToken) -> Result<()> {
        if let Some(hash) = token.hash() {
            self.shared.store.delete_session(&hash).await?;
        }
        Ok(())
    }

    /// The user a session token belongs to, while it lasts.
    pub async fn authenticate(&self, token: &SessionToken) -> Result<Option<User>> {
        let Some(hash) = token.hash() else {
            return Ok(None);
        };
        Ok(self
            .shared
            .store
            .session_user(&hash, self.shared.clock.now())
            .await?)
    }

    async fn open_session(&self, user: UserId) -> Result<SessionToken> {
        let secret = self.shared.random::<32>()?;
        let token = SessionToken(Lower(&secret).to_string());
        let hash: [u8; 32] = Sha256::digest(secret).into();

        let now = self.shared.clock.now();
        let expires = now
            .checked_add(self.shared.config.timing.session)
            .map_err(|_| Error::Unexpected)?;
        self.shared.store.delete_expired_sessions(now).await?;
        self.shared
            .store
            .create_session(&hash, user, now, expires)
            .await?;
        Ok(token)
    }
}

fn user_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Invalid::UserName.into());
    }
    Ok(name)
}

fn hash_password(password: &str, salt: &[u8; 16]) -> Result<String> {
    let salt = SaltString::encode_b64(salt).map_err(Error::Hash)?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(Error::Hash)?;
    Ok(hash.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

/// Build the decoy now, so the first login with an unknown name takes no
/// longer than any other.
pub(crate) async fn prepare() -> Result<()> {
    blocking(|| decoy().map(drop)).await
}

/// A hash no password matches, checked when the name is unknown.
fn decoy() -> Result<&'static str> {
    static DECOY: OnceLock<String> = OnceLock::new();
    if let Some(hash) = DECOY.get() {
        return Ok(hash);
    }
    let hash = hash_password("not a password anyone has", &[0x5a; 16])?;
    Ok(DECOY.get_or_init(|| hash))
}

/// Run CPU-bound work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    // The permit goes with the work: a caller that gives up leaves the hash
    // running, and it still counts.
    let permit = HASHERS.acquire().await.map_err(|_| Error::Unexpected)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| Error::Unexpected)?
}
