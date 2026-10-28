//! First-owner setup, login and browser sessions.
//!
//! Passwords are argon2id hashes, computed off the async threads and at
//! most [`HASHING`] at a time: each takes about 19 MiB, and logins need no
//! account, so a flood of them must queue rather than exhaust memory. A
//! session token is 32 random bytes the browser holds; the store keeps its
//! SHA-256, so a copied database holds no live session.
//!
//! A session lasts [`Timing::session`](crate::Timing::session) from its
//! last use. An account past its expiry is disabled: it cannot log in, and
//! a session of its that is presented is ended and refused. Those checks
//! hold on every request; the expiry job only clears what they would
//! refuse, so the table does not keep dead sessions.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use wellfed_api::{
    AuditAction, Credentials, Household, LoginFailure, PasswordChange, Role, Setup, SetupState,
    TimeFormat, Units, User, UserId,
};
use wellfed_formats::hex;
use wellfed_mirror::Zone;
use wellfed_store::NewUser;

use crate::accounts::tokens::session_only;
use crate::{AccountError, Caller, Error, InternalError, Invalid, Resource, Result, Service};

/// Password hashes computed at once.
const HASHING: usize = 2;
static HASHERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(HASHING);

/// The shortest password, with no other rule.
const PASSWORD_MIN: usize = 10;

/// How stale a session's last use may be before a request records it
/// again, so a busy page does not write on every request.
pub(crate) const TOUCH: Duration = Duration::from_secs(60);

/// The household a setup makes, besides its zone.
const HOUSEHOLD: &str = "Home";

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
    pub(crate) fn hash(&self) -> Option<[u8; 32]> {
        // Lowercase only: the one spelling this server writes.
        if !hex::is_lower_hex(&self.0, 64) {
            return None;
        }
        let bytes = hex::decode::<32>(&self.0)?;
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

    /// Whether setup is open, and the host's zone for the page to offer.
    pub async fn setup_state(&self) -> Result<SetupState> {
        Ok(SetupState {
            open: self.setup_open().await?,
            zone: self.shared.config.host_zone.clone(),
        })
    }

    /// Create the first owner and the household in its zone, and log the
    /// owner in. Refused once any user exists, even when two setups race.
    pub async fn setup(&self, setup: Setup) -> Result<(User, SessionToken)> {
        let name = user_name(&setup.name)?;
        password(&setup.password)?;
        let now = self.shared.clock.now();
        let zone = Zone::new(&setup.zone, now)?;
        if !self.setup_open().await? {
            return Err(AccountError::SetupClosed.into());
        }

        let hash = self.hash_password(setup.password).await?;
        let _one_at_a_time = self.owner_data.household.lock().await;
        let user = User {
            id: UserId(self.shared.new_id()?),
            name: name.to_owned(),
            role: Role::Owner,
            expires: None,
        };
        let household = Household {
            name: HOUSEHOLD.into(),
            units: Units::Grams,
            time_format: TimeFormat::H24,
            zone: zone.name().to_owned(),
        };
        let created = self
            .shared
            .store
            .create_first_owner(
                NewUser {
                    id: user.id,
                    name: &user.name,
                    password_hash: &hash,
                    role: user.role,
                    expires: None,
                    created: now,
                },
                &household,
            )
            .await?;
        if !created {
            return Err(AccountError::SetupClosed.into());
        }

        self.mirror.set_zone(zone);
        let token = self.open_session(user.id).await?;
        self.record_as(Some(&user), None, AuditAction::Setup).await;
        Ok((user, token))
    }

    /// Check a name and password from the client at `from` and open a
    /// session. An unknown name costs the same hash as a wrong password, so
    /// the hash's time does not reveal names; a wrong password for a real
    /// account also writes its audit row, a difference far inside the
    /// hash's own jitter at five tries a minute per name. A disabled
    /// account is told so only with its right password. Failed logins are
    /// limited per name and per address ([`AccountError::Limited`]).
    pub async fn login(
        &self,
        credentials: Credentials,
        from: IpAddr,
    ) -> Result<(User, SessionToken)> {
        let name = credentials.name.trim();
        let attempt = self
            .accounts
            .limits
            .attempt(name, from, self.shared.clock.now())
            .map_err(|limited| AccountError::Limited {
                retry_after: limited.retry_after,
            })?;

        let login = self.shared.store.login(name).await?;
        // A name with no password (an imported account's empty hash) costs
        // the decoy's hash like an unknown one, and matches nothing.
        let (user, hash, usable) = match login {
            Some(login) if PasswordHash::new(&login.password_hash).is_ok() => {
                (Some(login.user), login.password_hash, true)
            }
            Some(login) => (Some(login.user), decoy()?.to_owned(), false),
            None => (None, decoy()?.to_owned(), false),
        };
        let verified = blocking(move || Ok(verify_password(&credentials.password, &hash))).await?;
        let matches = usable && verified;

        let Some(user) = user else {
            return Err(AccountError::BadCredentials.into());
        };
        let failure = if !matches {
            Some(LoginFailure::BadPassword)
        } else if disabled(&user, self.shared.clock.now()) {
            Some(LoginFailure::Disabled)
        } else {
            None
        };
        if let Some(reason) = failure {
            let action = AuditAction::LoginFailed { from, reason };
            self.record_as(Some(&user), None, action).await;
            return Err(match reason {
                LoginFailure::BadPassword => AccountError::BadCredentials,
                LoginFailure::Disabled => AccountError::Disabled,
            }
            .into());
        }

        self.accounts
            .limits
            .succeeded(attempt, self.shared.clock.now());
        let token = self.open_session(user.id).await?;
        self.record_as(Some(&user), None, AuditAction::Login { from })
            .await;
        Ok((user, token))
    }

    pub async fn logout(&self, token: &SessionToken) -> Result<()> {
        if let Some(hash) = token.hash() {
            self.shared.store.delete_session(&hash).await?;
        }
        Ok(())
    }

    /// The user a session token belongs to, while it lasts, and its use
    /// recorded. A session of a disabled account is ended and refused
    /// [`AccountError::Disabled`].
    pub async fn authenticate(&self, token: &SessionToken) -> Result<Option<User>> {
        let Some(hash) = token.hash() else {
            return Ok(None);
        };
        let Some(session) = self.shared.store.session(&hash).await? else {
            return Ok(None);
        };

        let now = self.shared.clock.now();
        if later(session.last_used, self.session_lifetime())? <= now {
            self.shared.store.delete_session(&hash).await?;
            return Ok(None);
        }
        if disabled(&session.user, now) {
            self.shared.store.delete_session(&hash).await?;
            return Err(AccountError::Disabled.into());
        }
        if later(session.last_used, TOUCH)? <= now {
            self.shared.store.touch_session(&hash, now).await?;
        }
        Ok(Some(session.user))
    }

    /// Change the caller's own password, given the current one, and end
    /// their other sessions; `session`, the one they asked on, stays. Their
    /// tokens stay too: each is revoked on its own. It takes a login, as
    /// every password change does.
    pub async fn change_password(
        &self,
        caller: &Caller,
        session: Option<&SessionToken>,
        change: PasswordChange,
    ) -> Result<()> {
        session_only(caller)?;
        let user = &caller.user;
        password(&change.password)?;
        let store = &self.shared.store;
        let hash = store.password_hash(user.id).await?;
        let hash = hash.ok_or(Error::NotFound(Resource::User))?;
        let current = change.current;
        let matches = blocking(move || Ok(verify_password(&current, &hash))).await?;
        if !matches {
            return Err(AccountError::BadCredentials.into());
        }

        let hash = self.hash_password(change.password).await?;
        if !store.set_password(user.id, &hash).await? {
            return Err(Error::NotFound(Resource::User));
        }
        store
            .delete_sessions_of(user.id, session.and_then(SessionToken::hash).as_ref())
            .await?;
        let action = AuditAction::PasswordSet {
            account: user.id,
            name: user.name.clone(),
        };
        self.record(caller, action).await;
        Ok(())
    }

    async fn open_session(&self, user: UserId) -> Result<SessionToken> {
        let secret = self.shared.random::<32>()?;
        let token = SessionToken(hex::encode(&secret));
        let hash: [u8; 32] = Sha256::digest(secret).into();

        let now = self.shared.clock.now();
        self.shared.store.create_session(&hash, user, now).await?;
        Ok(token)
    }

    /// Hash a new password with a fresh salt.
    pub(crate) async fn hash_password(&self, password: String) -> Result<String> {
        let salt = self.shared.random::<16>()?;
        blocking(move || hash_password(&password, &salt)).await
    }
}

/// Whether `user`'s account is past its expiry at `now`.
pub(crate) fn disabled(user: &User, now: Timestamp) -> bool {
    user.expires.is_some_and(|expires| expires <= now)
}

/// `at` plus `after`, refused past the latest time the clock can show.
fn later(at: Timestamp, after: Duration) -> Result<Timestamp> {
    Ok(at
        .checked_add(after)
        .map_err(|_| InternalError::SessionExpiry)?)
}

pub(crate) fn user_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Invalid::UserName.into());
    }
    Ok(name)
}

pub(crate) fn password(password: &str) -> Result<()> {
    if password.chars().count() < PASSWORD_MIN {
        return Err(Invalid::Password.into());
    }
    Ok(())
}

pub(crate) fn hash_password(password: &str, salt: &[u8; 16]) -> Result<String> {
    let salt = SaltString::encode_b64(salt).map_err(InternalError::Hash)?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(InternalError::Hash)?;
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
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    // The permit goes with the work: a caller that gives up leaves the hash
    // running, and it still counts.
    let permit = HASHERS
        .acquire()
        .await
        .map_err(|_| InternalError::Hashing)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| InternalError::Hashing)?
}
