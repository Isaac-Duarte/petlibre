//! The owner's server domain logic ([API design]): household users, the
//! adopted feeders and one connection task per feeder. It sees neither HTTP
//! nor SQL: the server crate maps requests onto these methods, and
//! `wellfed-store` holds the data.
//!
//! - **Feeders** are adopted by redeeming an invite with a key the server
//!   makes for that feeder, so the feeder's history names the server. The
//!   key is sealed with the config's key before it is stored.
//! - **Each feeder's task** ([`feeder`]) keeps a session open, reconnecting
//!   with jittered backoff. On every connect it first subscribes after the
//!   newest event it holds, so its own outages lose nothing, then sets the
//!   feeder's clock (the only time source until the feeder keeps its own),
//!   and records every event and sample as the feeder sent it.
//! - **Commands** reach the task through a channel. One that is not
//!   answered in time is [`Error::Unknown`], never a failure: the feeder may
//!   have run it, and a retry under the same request id gets its first
//!   answer.
//! - **Live updates** go through the [`Hub`], which the server's event
//!   stream reads.
//!
//! Time and randomness come in through [`Clock`] and [`Random`], so tests
//! can fix them.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod feeder;
mod feeders;
mod hub;
mod keys;
mod users;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_api::{FeederId, FeederRefusal, Settings};
use wellfed_client::api::InviteText;
use wellfed_store::Store;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

pub use hub::{Hub, Resume, Stamped, StreamId, Subscription};
pub use users::SessionToken;

/// A device request id.
pub(crate) type RequestId = wellfed_proto::api::RequestId;

/// A client's `Idempotency-Key`: a retried write carries the same one.
pub type IdempotencyKey = [u8; 16];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] wellfed_store::Error),
    #[error("setup is closed: an account exists")]
    SetupClosed,
    #[error("wrong name or password")]
    BadCredentials,
    #[error("{0}")]
    Invalid(#[from] Invalid),
    #[error("{0}")]
    Invite(#[from] InviteText),
    #[error("no such feeder")]
    NotFound,
    #[error("that feeder is already adopted")]
    AlreadyAdopted,
    #[error("the feeder dropped the invite: unknown, used or expired")]
    InviteExpired,
    #[error("reach the feeder: {0}")]
    Unreachable(wellfed_connect::Error),
    #[error("the server has no session with the feeder")]
    Offline,
    #[error("no answer in time: retry with the same key")]
    Unknown,
    #[error("the feeder refused: {0:?}")]
    Refused(FeederRefusal),
    #[error("the settings moved on to generation {}", .0.generation)]
    Stale(Settings),
    #[error("the feeder sent an answer that does not fit the request")]
    Unexpected,
    #[error("stored event {0} does not decode")]
    Undecodable(u64),
    #[error("the system random generator failed")]
    Random,
    #[error("hash a password: {0}")]
    Hash(argon2::password_hash::Error),
    #[error("a feeder key does not open with this server's key")]
    Unseal,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A field that failed the server's checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("a user name is 1-64 characters")]
    UserName,
    #[error("a password is at least 8 characters")]
    Password,
    #[error("a feeder name is 1-64 characters")]
    FeederName,
    #[error("the address is not an IP or IP:port")]
    Address,
    #[error("give the feeder's address: discovery is off")]
    AddressRequired,
    #[error("the volume is 0-100")]
    Volume,
    #[error("a page holds 1-500 events")]
    Limit,
    #[error("the server's label is 1-16 bytes with no NUL")]
    Label,
    #[error("an event cursor is at most 2^63 - 1")]
    Cursor,
}

/// The wall clock.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;
}

pub struct SystemClock;

impl Clock for SystemClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place a server crate reads the wall clock"
    )]
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// Secret bytes: keys, session tokens, request ids, salts.
pub trait Random: Send + Sync + 'static {
    fn fill(&self, out: &mut [u8]) -> Result<()>;
}

pub struct SystemRandom;

impl Random for SystemRandom {
    fn fill(&self, out: &mut [u8]) -> Result<()> {
        getrandom::fill(out).map_err(|_| Error::Random)
    }
}

/// Whether a feeder whose address fails is looked up over mDNS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discovery {
    Mdns,
    Off,
}

pub struct Config {
    /// Seals the server's feeder keys at rest.
    pub key: [u8; 32],
    /// The zone the feeders' clocks are set to.
    pub zone: TimeZone,
    /// The label the server enrolls under on each feeder (1-16 bytes).
    pub label: String,
    pub discovery: Discovery,
    pub timing: Timing,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("label", &self.label)
            .field("discovery", &self.discovery)
            .field("timing", &self.timing)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// The first reconnect delay, doubled after each failure.
    pub retry_min: Duration,
    pub retry_max: Duration,
    /// How often a connected feeder's clock is set again.
    pub time_sync: Duration,
    /// How long a caller waits for a command's answer: the connection's
    /// own answer timeout, and some room to queue behind another command.
    pub answer: Duration,
    /// How long a login lasts.
    pub session: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            retry_min: Duration::from_secs(1),
            retry_max: Duration::from_secs(60),
            time_sync: Duration::from_secs(6 * 3600),
            answer: wellfed_connect::ANSWER + Duration::from_secs(5),
            session: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// What every task shares.
struct Shared {
    store: Store,
    config: Config,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Random>,
    hub: Hub,
}

impl Shared {
    fn random<const N: usize>(&self) -> Result<[u8; N]> {
        let mut out = [0; N];
        self.random.fill(&mut out)?;
        Ok(out)
    }

    /// A new time-ordered id (UUIDv7), its time from the service's clock and
    /// its counter and random bits from its generator, so a test that fixes
    /// both gets the same ids every run.
    fn new_id(&self) -> Result<Uuid> {
        Ok(time_ordered(self.clock.now(), &self.random()?))
    }
}

fn time_ordered(at: Timestamp, bits: &[u8; 10]) -> Uuid {
    let millis = at.as_millisecond().unsigned_abs();
    uuid::Builder::from_unix_timestamp_millis(millis, bits).into_uuid()
}

/// The running service: cheap to clone, one per server.
#[derive(Clone)]
pub struct Service {
    shared: Arc<Shared>,
    feeders: Arc<Mutex<HashMap<FeederId, feeder::Handle>>>,
    adopting: Arc<tokio::sync::Mutex<()>>,
    cancel: CancellationToken,
    tasks: TaskTracker,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").finish_non_exhaustive()
    }
}

impl Service {
    /// Start a task for every adopted feeder.
    pub async fn start(
        store: Store,
        config: Config,
        clock: Arc<dyn Clock>,
        random: Arc<dyn Random>,
    ) -> Result<Self> {
        let label = config.label.as_bytes();
        if label.is_empty() || label.len() > 16 || label.contains(&0) {
            return Err(Invalid::Label.into());
        }
        let hub = Hub::new(clock.now());
        let service = Self {
            shared: Arc::new(Shared {
                store,
                config,
                clock,
                random,
                hub,
            }),
            feeders: Arc::new(Mutex::new(HashMap::new())),
            adopting: Arc::new(tokio::sync::Mutex::new(())),
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
        };
        users::prepare().await?;
        for row in service.shared.store.feeders().await? {
            service.spawn(&row)?;
        }
        Ok(service)
    }

    /// Stop every task and wait for them, then close the store.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
        self.shared.store.close().await;
    }

    pub fn hub(&self) -> &Hub {
        &self.shared.hub
    }

    /// How long a login lasts.
    pub fn session_lifetime(&self) -> Duration {
        self.shared.config.timing.session
    }

    /// A fresh request id.
    pub(crate) fn request_id(&self) -> Result<RequestId> {
        self.shared.random()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_carries_the_clocks_time_and_the_random_bits() {
        let at = Timestamp::from_millisecond(1_790_000_000_123).unwrap();

        let id = time_ordered(at, &[0xab; 10]);

        assert_eq!(id.get_version_num(), 7);
        let (seconds, nanos) = id.get_timestamp().unwrap().to_unix();
        assert_eq!((seconds, nanos), (1_790_000_000, 123_000_000));
        assert_eq!(id, time_ordered(at, &[0xab; 10]), "fixed inputs, fixed id");
        assert_ne!(id, time_ordered(at, &[0xcd; 10]));
    }
}
