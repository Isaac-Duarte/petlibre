//! The service on a temporary store, with fixed time and seeded random.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{AdoptFeeder, ClientRole, Connection, EventBody, EventQuery, FeederId};
use wellfed_service::{
    Backups, Clock, Config, Diagnostics, Discovery, ReleaseKeys, ReleasesConfig, Service,
    StockConfig, StockPins, Timing,
};
use wellfed_store::Store;
use tempfile::TempDir;

use crate::feeder::{self, Running};
use crate::fixed::{FixedClock, SeededRandom};
use crate::until;

/// The config every test server starts with: no discovery, reconnects
/// fast enough for a test to watch, no trusted release key and no release
/// fetching, samples written within a tenth of a second and kept raw
/// forever, and seven backups. Its release cache and backup directories
/// are placeholders: a [`TestService`] keeps both beside its database.
pub fn config() -> Config {
    Config {
        key: [0x42; 32],
        host_zone: Some("UTC".into()),
        label: "test-server".into(),
        discovery: Discovery::Off,
        timing: Timing {
            retry_min: Duration::from_millis(50),
            retry_max: Duration::from_millis(400),
            time_sync: Duration::from_secs(3600),
            answer: Duration::from_secs(12),
            sample_wait: Duration::from_millis(100),
            session: Duration::from_secs(3600),
        },
        // The diagnostics' own tests turn them on: elsewhere their reads
        // would share the lanes and the seeded ids with what is tested.
        diagnostics: Diagnostics::Off,
        releases: ReleasesConfig {
            dir: PathBuf::from("releases"),
            keys: ReleaseKeys::default(),
            fetch: None,
            stock: StockConfig {
                dir: PathBuf::from("stock"),
                pins: StockPins::STOCK,
            },
        },
        raw_sample_days: None,
        backups: Backups {
            dir: PathBuf::from("backups"),
            keep: NonZeroU32::new(7).expect("seven is not zero"),
        },
    }
}

/// A database in a temporary directory, deleted with it.
#[derive(Debug)]
pub struct TempStore(TempDir);

impl TempStore {
    pub fn new() -> Self {
        Self(tempfile::tempdir().expect("make a temporary directory"))
    }

    pub fn path(&self) -> PathBuf {
        self.0.path().join("server.db")
    }

    /// The release cache, as the server keeps it in its data directory.
    pub fn releases(&self) -> PathBuf {
        self.0.path().join("releases")
    }

    /// The stock backups, as the server keeps them in its data directory.
    pub fn stock(&self) -> PathBuf {
        self.0.path().join("stock")
    }

    /// The backups, as the server keeps them in its data directory.
    pub fn backups(&self) -> PathBuf {
        self.0.path().join("backups")
    }

    /// The data directory itself.
    pub fn dir(&self) -> &std::path::Path {
        self.0.path()
    }

    /// Open the database, as the server does at the fixed time.
    pub async fn open(&self) -> Store {
        Store::open(&self.path(), FixedClock.now())
            .await
            .expect("open the test database")
    }
}

impl Default for TempStore {
    fn default() -> Self {
        Self::new()
    }
}

/// The service, running on its own temporary store.
#[derive(Debug)]
pub struct TestService {
    store: TempStore,
    service: Service,
}

impl TestService {
    /// The service with [`config`] on a fresh store.
    pub async fn start() -> Self {
        Self::start_with(config()).await
    }

    pub async fn start_with(config: Config) -> Self {
        Self::open(TempStore::new(), config).await
    }

    /// The service with [`config`] on a fresh store, reading `clock`.
    pub async fn start_with_clock(clock: Arc<dyn Clock>) -> Self {
        Self::open_with_clock(TempStore::new(), config(), clock).await
    }

    /// The service with `config` on `store`, which may hold a previous
    /// run's data, its release cache included. Each start draws from a
    /// seed of its own.
    pub async fn open(store: TempStore, config: Config) -> Self {
        Self::open_with_clock(store, config, Arc::new(FixedClock)).await
    }

    async fn open_with_clock(store: TempStore, mut config: Config, clock: Arc<dyn Clock>) -> Self {
        config.releases.dir = store.releases();
        config.releases.stock.dir = store.stock();
        config.backups.dir = store.backups();
        let service = Service::start(
            store.open().await,
            config,
            clock,
            Arc::new(SeededRandom::fresh()),
        )
        .await
        .expect("start the service");
        Self { store, service }
    }

    /// Stop, keeping the database for [`Self::reopen`].
    pub async fn stop(self) -> TempStore {
        self.service.shutdown().await;
        self.store
    }

    /// The service again on a stopped one's database, with [`config`].
    pub async fn reopen(store: TempStore) -> Self {
        Self::open(store, config()).await
    }

    pub fn service(&self) -> &Service {
        &self.service
    }

    pub fn store(&self) -> &TempStore {
        &self.store
    }

    /// Adopt `sim` by an invite for the server, at its address.
    pub async fn adopt(&self, sim: &Running) -> FeederId {
        self.adopt_at(sim, &sim.address.to_string()).await
    }

    /// Adopt `sim` by an invite for the server, reaching it at `address`
    /// (a relay in front of it, say).
    pub async fn adopt_at(&self, sim: &Running, address: &str) -> FeederId {
        self.adopt_as(sim, address, ClientRole::Server).await
    }

    /// Adopt `sim` at `address` by an invite for `role`: an owner's key is
    /// one the feeder lets set its time settings.
    pub async fn adopt_as(&self, sim: &Running, address: &str, role: ClientRole) -> FeederId {
        let feeder = self
            .service
            .adopt(AdoptFeeder {
                invite: feeder::invite(sim, role).await,
                name: "kitchen".into(),
                address: Some(address.into()),
            })
            .await
            .expect("the service adopts the simulator");
        assert_eq!(feeder.role, role);
        feeder.id
    }

    /// Wait until the feeder's link is `want`.
    pub async fn link(&self, id: FeederId, want: Connection) {
        until(&format!("the feeder's link is {want:?}"), || async {
            let feeders = self.service.feeders().await.expect("list the feeders");
            feeders.iter().any(|f| f.id == id && f.connection == want)
        })
        .await;
    }

    /// Wait until the server holds the feeder's events through `seq`.
    pub async fn synced(&self, id: FeederId, seq: u64) {
        until(
            &format!("the server holds events through {seq}"),
            || async {
                let detail = self.service.feeder(id).await.expect("read the feeder");
                detail.feeder.synced.is_some_and(|synced| synced >= seq)
            },
        )
        .await;
    }

    /// The bodies of the first page of the feeder's history.
    pub async fn bodies(&self, id: FeederId) -> Vec<EventBody> {
        let page = self
            .service
            .events(id, EventQuery::default())
            .await
            .expect("read the feeder's history");
        page.events.into_iter().map(|event| event.body).collect()
    }

    /// Stop the service and its tasks.
    pub async fn shutdown(self) {
        self.service.shutdown().await;
    }
}
