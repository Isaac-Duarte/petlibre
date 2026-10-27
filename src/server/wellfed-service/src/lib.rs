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
//!   through. Failed logins are limited per name and per address, and
//!   never lock an account.
//! - **The audit log** is the command journal and the account changes,
//!   each user's own or everyone's for an owner.
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
//! - **Phones** of owners and members keep a key of their own on each
//!   feeder, from an invite the server makes and records as their
//!   account's and token's; an account change that takes their direct
//!   access away revokes those keys on every feeder, retried at each
//!   reconnect until it lands. The feeders' clients, Wi-Fi, restart and
//!   removal go through the journal like every write, and [`parity`]
//!   gives every device operation its route or the reason it has none.
//! - **Feeds** are journalled writes answered with the feeder's number for
//!   the feed, and settled from the feeder's history: its `Fed` event. An
//!   uncertain feed is never sent again.
//! - **Live updates** go through the [`Hub`], which the server's event
//!   stream reads: each feeder's mirror broadcast is forwarded into it,
//!   with each change to a journal entry, and the hub assigns the stream
//!   ids.
//! - **Owner data** is the pets, the foods and the refills the feeders
//!   do not hold. Each feeder serves one food at a time, with a history of
//!   when each started, so a past feed's calories use the food it served.
//! - **Releases** are `wellfed-releases`' cache: uploaded or fetched,
//!   verified the same way, and kept until an owner installs one.
//! - **Samples** are read raw or by the hour; the jobs keep the hourly
//!   rollups and drop raw rows past the owner's retention.
//! - **Backups** are copies of the database in the data directory, taken
//!   daily and on an owner's request, without the sealing key.
//! - **Installs** run one at a time per server, in the background on the
//!   feeder's long-operation lane, so feeds and status reads keep working;
//!   their progress and their end reach the stream. "Restore stock" is
//!   one: it installs the feeder's own stock firmware from the verified
//!   backup the server keeps in its data directory, signed by the owner.
//! - **Background jobs** have their bodies here, each a method that takes
//!   the time it runs at; [`Service::run_job`] runs one and records its
//!   run. The server's supervisor decides when.
//!
//! [`Error`] holds what every domain shares: a field that failed its
//! checks, a [`Resource`] that is not there, the feeder's own refusal and
//! the server's own failures ([`InternalError`]). Each domain's other
//! refusals have their own enum ([`AccountError`], [`FeederError`],
//! [`ClientError`], [`CommandError`], [`InstallError`],
//! [`OwnerDataError`]), every variant with its one API code, and
//! [`Error::reply`] says how each is answered. Time and randomness come in
//! through [`Clock`] and [`Random`], so tests can fix them.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod accounts;
mod config;
mod error;
mod feeders;
mod hub;
mod operations;
mod owner_data;
mod page;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub use accounts::{AccountError, BearerToken, Caller, Permission, SessionToken, can, reset_owner};
pub use config::{Config, Timing};
pub use error::{Error, InternalError, Invalid, Reply, Resource, Result};
pub use feeders::{
    ClientError, CommandError, FeederError, LogFollow, Method, NoRoute, Parity, parity,
};
pub use hub::{Hub, Resume, Stamped, StreamId, Subscription};
use jiff::Timestamp;
pub use operations::{BackupError, Backups, EXPORT_CHUNK, ExportError, InstallError};
pub use owner_data::OwnerDataError;
use wellfed_mirror::Mirror;
pub use wellfed_mirror::{
    Clock, Diagnostics, Discovery, Random, RandomFailed, SystemClock, SystemRandom, ZoneError,
};
use wellfed_releases::Releases;
pub use wellfed_releases::{
    Added as ReleaseAdded, Config as ReleasesConfig, Error as ReleaseError, Keys as ReleaseKeys,
    MAX_FILE as RELEASE_MAX, Source as ReleaseFeed, StockConfig, StockPins,
};
use wellfed_store::Store;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

/// A device request id.
pub(crate) type RequestId = wellfed_mirror::RequestId;

/// A client's `Idempotency-Key`: a retried write carries the same one.
pub type IdempotencyKey = [u8; 16];

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
    accounts: accounts::State,
    feeders: feeders::State,
    owner_data: owner_data::State,
    operations: operations::State,
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
            accounts: accounts::State::default(),
            feeders: feeders::State::default(),
            owner_data: owner_data::State::default(),
            operations: operations::State::default(),
            cancel,
            tasks: TaskTracker::new(),
        };

        accounts::sessions::prepare().await?;
        service.interrupt_commands().await?;
        service.interrupt_installs().await?;
        service.load_zone().await?;
        // Before any feeder task ingests, so none writes beside a stale
        // build of the projections.
        service.mirror.refresh_projections().await?;
        service.mirror.refresh_samples().await?;
        // Marked before the feeders connect, which revokes them.
        let now = service.shared.clock.now();
        service.shared.store.reconcile_phones(now).await?;
        for row in service.shared.store.feeders().await? {
            if row.archived.is_none() {
                service.spawn(&row).await?;
            }
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
