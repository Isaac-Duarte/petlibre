//! The owner's server domain logic ([API design]): household users, the
//! adopted feeders and the commands sent to them. It sees neither HTTP,
//! SQL nor a feeder session: the server crate maps requests onto these
//! methods, `wellfed-store` holds the data, and `wellfed-mirror` runs
//! each feeder's session.
//!
//! - **Accounts** are owners, members and viewers; [`can`] holds what each
//!   may do, and the server asks it per route. A session lasts from its
//!   last use, and an account past its expiry is disabled. A phone or the
//!   CLI authenticates with a bearer token instead: its user's, with no
//!   role of its own, so a [`Caller`] is a user and the token they came
//!   through.
//! - **The household** holds the zone every feeder keeps, as a POSIX rule
//!   the mirror gives each one.
//! - **Feeders** are adopted by redeeming an invite with a key the server
//!   makes for that feeder, so the feeder's history names the server. The
//!   key is sealed with the config's key before it is stored, and opened
//!   only to hand it to the mirror.
//! - **Commands** go through the mirror. One that is not answered in time
//!   is [`CommandError::Unknown`], never a failure: the feeder may have run
//!   it. Every write is recorded in the command journal before it is sent,
//!   so a retry under the same key gets its first answer from the journal,
//!   however many writes the feeder has seen since.
//! - **Live updates** go through the [`Hub`], which the server's event
//!   stream reads: each feeder's mirror broadcast is forwarded into it,
//!   and the hub assigns the stream ids.
//! - **Releases** are `wellfed-releases`' cache: uploaded or fetched,
//!   verified the same way, and kept until an owner installs one.
//! - **Background jobs** have their bodies here, each a method that takes
//!   the time it runs at; [`Service::run_job`] runs one and records its
//!   run. The server's supervisor decides when.
//!
//! Each domain has its own error enum ([`AccountError`],
//! [`TokenError`], [`HouseholdError`], [`FeederError`], [`CommandError`]),
//! wrapped in [`Error`]. Time and randomness come in
//! through [`Clock`] and [`Random`], so tests can fix them.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod commands;
mod error;
mod feeders;
mod health;
mod household;
mod hub;
mod jobs;
mod journal;
mod keys;
mod permissions;
mod releases;
mod sessions;
mod tokens;
mod updates;
mod users;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{Connection as Link, FeederId};
use wellfed_mirror::Mirror;
use wellfed_releases::Releases;
use wellfed_store::Store;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

pub use error::{
    AccountError, CommandError, Error, FeederError, HouseholdError, Invalid, Result, TokenError,
};
pub use hub::{Hub, Resume, Stamped, StreamId, Subscription};
pub use permissions::{Permission, can};
pub use wellfed_mirror::{
    Clock, Discovery, Random, RandomFailed, SystemClock, SystemRandom, ZoneError,
};
pub use wellfed_releases::{
    Added as ReleaseAdded, Config as ReleasesConfig, Error as ReleaseError, Keys as ReleaseKeys,
    MAX_FILE as RELEASE_MAX, Source as ReleaseFeed,
};
pub use sessions::SessionToken;
pub use tokens::{BearerToken, Caller};
pub use users::reset_owner;

/// A device request id.
pub(crate) type RequestId = wellfed_mirror::RequestId;

/// A client's `Idempotency-Key`: a retried write carries the same one.
pub type IdempotencyKey = [u8; 16];

pub struct Config {
    /// Seals the server's feeder keys at rest.
    pub key: [u8; 32],
    /// The host's IANA zone, offered at setup as the household's.
    pub host_zone: Option<String>,
    /// The label the server enrolls under on each feeder (1-16 bytes).
    pub label: String,
    pub discovery: Discovery,
    pub timing: Timing,
    /// The release cache, the keys a release may be signed with, and
    /// whether releases are fetched.
    pub releases: ReleasesConfig,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("host_zone", &self.host_zone)
            .field("label", &self.label)
            .field("discovery", &self.discovery)
            .field("timing", &self.timing)
            .field("releases", &self.releases)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// What the mirror needs of it: no key, no label, no logins.
    fn mirror(&self) -> wellfed_mirror::Config {
        let timing = self.timing;
        wellfed_mirror::Config {
            discovery: self.discovery,
            timing: wellfed_mirror::Timing {
                retry_min: timing.retry_min,
                retry_max: timing.retry_max,
                time_sync: timing.time_sync,
                answer: timing.answer,
            },
        }
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
    /// How long a login lasts after its last use.
    pub session: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        let mirror = wellfed_mirror::Timing::default();
        Self {
            retry_min: mirror.retry_min,
            retry_max: mirror.retry_max,
            time_sync: mirror.time_sync,
            answer: mirror.answer,
            session: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// What every use case shares.
struct Shared {
    store: Store,
    config: Config,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Random>,
    hub: Arc<Hub>,
    /// When this run started, by the service's clock.
    started: Timestamp,
}

impl Shared {
    fn random<const N: usize>(&self) -> Result<[u8; N]> {
        let mut out = [0; N];
        self.random.fill(&mut out)?;
        Ok(out)
    }

    /// A new time-ordered id (UUIDv7), its time from the service's clock and
    /// its 74 random bits from its random source, so a test that fixes
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
    mirror: Mirror,
    releases: Arc<Releases>,
    /// Each feeder's link as the stream last told it, so a list never
    /// shows a state the stream has not.
    links: Arc<Mutex<HashMap<FeederId, Link>>>,
    adopting: Arc<tokio::sync::Mutex<()>>,
    /// Household changes run one at a time, so the stored zone and the
    /// mirror's are always the last change's.
    household: Arc<tokio::sync::Mutex<()>>,
    cancel: CancellationToken,
    tasks: TaskTracker,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").finish_non_exhaustive()
    }
}

impl Service {
    /// Start mirroring every adopted feeder.
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

        let releases = Releases::open(config.releases.clone()).await?;
        let cancel = CancellationToken::new();
        let mirror = Mirror::new(
            store.clone(),
            config.mirror(),
            Arc::clone(&clock),
            Arc::clone(&random),
            cancel.child_token(),
        );
        let started = clock.now();
        let hub = Arc::new(Hub::new(started));
        let service = Self {
            shared: Arc::new(Shared {
                store,
                config,
                clock,
                random,
                hub,
                started,
            }),
            mirror,
            releases: Arc::new(releases),
            links: Arc::new(Mutex::new(HashMap::new())),
            adopting: Arc::new(tokio::sync::Mutex::new(())),
            household: Arc::new(tokio::sync::Mutex::new(())),
            cancel,
            tasks: TaskTracker::new(),
        };

        sessions::prepare().await?;
        service.interrupt_commands().await?;
        service.load_zone().await?;
        // Before any feeder task ingests, so none writes beside a stale
        // build of the projections.
        service.mirror.refresh_projections().await?;
        for row in service.shared.store.feeders().await? {
            service.spawn(&row).await?;
        }
        Ok(service)
    }

    /// Stop every task and wait for them, then close the store.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        self.mirror.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
        self.shared.store.close().await;
    }

    pub fn hub(&self) -> &Hub {
        &self.shared.hub
    }

    /// How long a login lasts after its last use.
    pub fn session_lifetime(&self) -> Duration {
        self.shared.config.timing.session
    }

    /// A fresh request id.
    pub(crate) fn request_id(&self) -> Result<RequestId> {
        self.shared.random()
    }

    fn links(&self) -> MutexGuard<'_, HashMap<FeederId, Link>> {
        lock(&self.links)
    }
}

/// Insertions are whole, so a poisoned map is still consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
