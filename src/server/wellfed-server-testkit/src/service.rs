//! The service on a temporary store, with fixed time and seeded random.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString};
use jiff::tz::TimeZone;
use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, Credentials, EventBody, EventQuery, FeederId, Role, UserId,
};
use wellfed_service::{Config, Discovery, Service, Timing};
use wellfed_store::{NewUser, Store};
use tempfile::TempDir;
use uuid::Uuid;

use crate::feeder::{self, Running};
use crate::fixed::{FixedClock, SeededRandom};
use crate::until;

/// The config every test server starts with: no discovery, and reconnects
/// fast enough for a test to watch.
pub fn config() -> Config {
    Config {
        key: [0x42; 32],
        zone: TimeZone::UTC,
        label: "test-server".into(),
        discovery: Discovery::Off,
        timing: Timing {
            retry_min: Duration::from_millis(50),
            retry_max: Duration::from_millis(400),
            time_sync: Duration::from_secs(3600),
            answer: Duration::from_secs(12),
            session: Duration::from_secs(3600),
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

    /// Open the database, as the server does at the fixed time.
    pub async fn open(&self) -> Store {
        Store::open(&self.path(), FixedClock.now())
            .await
            .expect("open the test database")
    }

    /// A fresh database whose one account is a member who logs in with
    /// `login`. No route makes a member yet, so the store does, as setup
    /// would with the member's role; once one does, this goes through it.
    pub async fn with_member(login: &Credentials) -> Self {
        let temp = Self::new();
        let store = temp.open().await;
        let salt = SaltString::encode_b64(&[0x11; 16]).expect("sixteen bytes make a salt");
        let hash = Argon2::default()
            .hash_password(login.password.as_bytes(), &salt)
            .expect("hash the member's password")
            .to_string();
        let created = store
            .create_first_owner(NewUser {
                id: UserId(Uuid::from_u128(2)),
                name: &login.name,
                password_hash: &hash,
                role: Role::Member,
                created: FixedClock.now(),
            })
            .await
            .expect("store the member");
        assert!(created, "a fresh database takes its first account");
        store.close().await;
        temp
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

    /// The service with `config` on `store`, which may hold a previous
    /// run's data. Each start draws from a seed of its own.
    pub async fn open(store: TempStore, config: Config) -> Self {
        let service = Service::start(
            store.open().await,
            config,
            Arc::new(FixedClock),
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
        let feeder = self
            .service
            .adopt(AdoptFeeder {
                invite: feeder::invite(sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(address.into()),
            })
            .await
            .expect("the service adopts the simulator");
        assert_eq!(feeder.role, ClientRole::Server);
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
