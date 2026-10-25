//! The service against the simulated feeder over real TCP: adoption, the
//! connect sequence, commands, history sync across the server's outages,
//! and the live hub.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_api::{
    AdoptFeeder, ClientRole, Connection as Link, Credentials, EventBody, EventKind, EventQuery,
    FeederId, Generation, Role, SettingsPatch, Source, StreamEvent, User, UserId,
};
use wellfed_client::api::Invite;
use wellfed_connect::Connection;
use wellfed_core::noise::Keypair;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::{Relay, Running};
use wellfed_proto::api::{Op, Outcome, Role as DeviceRole};
use wellfed_service::{
    Clock, Config, Discovery, Error, IdempotencyKey, Invalid, Resume, Service, SystemRandom, Timing,
};
use wellfed_store::Store;
use tempfile::TempDir;
use tokio::task::LocalSet;
use uuid::Uuid;

const OWNER: [u8; 32] = [0x21; 32];
const WAIT: Duration = Duration::from_secs(10);

/// A fixed instant, inside the feeder clock's accepted range.
struct Fixed;

const NOW_MS: i64 = 1_790_000_000_000;

impl Clock for Fixed {
    fn now(&self) -> Timestamp {
        Timestamp::from_millisecond(NOW_MS).unwrap()
    }
}

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

fn owner() -> Keypair {
    Keypair::from_secret(OWNER)
}

/// The household user a write comes from.
fn ada() -> User {
    user(1, "ada")
}

fn user(n: u128, name: &str) -> User {
    User {
        id: UserId(Uuid::from_u128(n)),
        name: name.into(),
        role: Role::Owner,
    }
}

fn config() -> Config {
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

struct Server {
    dir: TempDir,
    service: Service,
}

impl Server {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let service = Self::open(&dir).await;
        Self { dir, service }
    }

    async fn open(dir: &TempDir) -> Service {
        let store = Store::open(&dir.path().join("server.db")).await.unwrap();
        Service::start(store, config(), Arc::new(Fixed), Arc::new(SystemRandom))
            .await
            .unwrap()
    }

    /// Stop, keeping the database.
    async fn stop(self) -> TempDir {
        self.service.shutdown().await;
        self.dir
    }

    async fn reopen(dir: TempDir) -> Self {
        let service = Self::open(&dir).await;
        Self { dir, service }
    }
}

/// An owner's session with the simulator.
async fn open_owner(sim: &Running) -> Connection {
    Connection::open(&owner(), sim.identity, sim.address, [0x99; 32])
        .await
        .unwrap()
}

/// The feeder answers a repeated write id from memory, so each invite
/// takes a new one.
async fn invite(sim: &Running, role: DeviceRole) -> String {
    static IDS: AtomicU8 = AtomicU8::new(0x80);
    let id = [IDS.fetch_add(1, Ordering::Relaxed); 16];
    let mut connection = open_owner(sim).await;
    let answer = connection.ask(id, Op::CreateInvite { role }).await.unwrap();
    let Outcome::Invite(invite) = answer.outcome() else {
        panic!("{answer:?}");
    };
    Invite {
        feeder: sim.identity,
        id: invite.id,
        secret: invite.secret,
    }
    .text()
}

async fn adopt(server: &Server, sim: &Running) -> FeederId {
    let feeder = server
        .service
        .adopt(AdoptFeeder {
            invite: invite(sim, DeviceRole::Server).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        })
        .await
        .unwrap();
    assert_eq!(feeder.role, ClientRole::Server);
    feeder.id
}

/// Wait until the feeder's link is `want`.
async fn link(service: &Service, id: FeederId, want: Link) {
    tokio::time::timeout(WAIT, async {
        loop {
            let feeders = service.feeders().await.unwrap();
            if feeders.iter().any(|f| f.id == id && f.connection == want) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the feeder never became {want:?}"));
}

/// Wait until the server holds events through `seq`.
async fn synced(service: &Service, id: FeederId, seq: u64) {
    tokio::time::timeout(WAIT, async {
        loop {
            let feeder = service.feeder(id).await.unwrap().feeder;
            if feeder.synced.is_some_and(|synced| synced >= seq) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the server never synced through {seq}"));
}

async fn bodies(service: &Service, id: FeederId) -> Vec<EventBody> {
    let page = service.events(id, EventQuery::default()).await.unwrap();
    page.events.into_iter().map(|event| event.body).collect()
}

#[test]
fn the_first_owner_sets_up_once_and_logs_in() {
    local(async {
        let server = Server::start().await;
        let service = &server.service;
        assert!(service.setup_open().await.unwrap());

        let weak = Credentials {
            name: "ada".into(),
            password: "short".into(),
        };
        assert!(matches!(
            service.setup(weak).await,
            Err(Error::Invalid(Invalid::Password))
        ));
        let credentials = Credentials {
            name: " ada ".into(),
            password: "correct horse".into(),
        };
        let (user, token) = service.setup(credentials.clone()).await.unwrap();
        assert_eq!((user.name.as_str(), user.role), ("ada", Role::Owner));
        assert!(!service.setup_open().await.unwrap());
        assert!(matches!(
            service.setup(credentials.clone()).await,
            Err(Error::SetupClosed)
        ));
        assert_eq!(
            service.authenticate(&token).await.unwrap(),
            Some(user.clone())
        );

        let wrong = Credentials {
            password: "wrong horse".into(),
            ..credentials.clone()
        };
        assert!(matches!(
            service.login(wrong).await,
            Err(Error::BadCredentials)
        ));
        let nobody = Credentials {
            name: "bob".into(),
            ..credentials.clone()
        };
        assert!(matches!(
            service.login(nobody).await,
            Err(Error::BadCredentials)
        ));
        let (again, second) = service.login(credentials).await.unwrap();
        assert_eq!(again, user);

        service.logout(&token).await.unwrap();
        assert_eq!(service.authenticate(&token).await.unwrap(), None);
        assert_eq!(service.authenticate(&second).await.unwrap(), Some(user));
        server.service.shutdown().await;
    });
}

#[test]
fn an_adopted_feeder_connects_syncs_its_clock_and_mirrors_its_history() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let server = Server::start().await;
        let id = adopt(&server, &sim).await;
        link(&server.service, id, Link::Online).await;

        let detail = server.service.feeder(id).await.unwrap();
        let status = detail.status.unwrap();
        assert!(status.live);
        assert_eq!(
            status
                .status
                .clock
                .map(|clock| clock.as_millisecond() / 1000),
            Some(NOW_MS / 1000),
            "the server set the feeder's clock on connect"
        );

        // Boot, the server's enrolment, the clock sync.
        synced(&server.service, id, status.status.last_event.unwrap()).await;
        let history = bodies(&server.service, id).await;
        assert!(matches!(history[0], EventBody::Boot { .. }));
        assert!(history.iter().any(|body| matches!(
            body,
            EventBody::Enrolled {
                role: ClientRole::Server,
                ..
            }
        )));
        assert!(
            history
                .iter()
                .any(|body| matches!(body, EventBody::ClockSync { .. }))
        );

        let again = server.service.adopt(AdoptFeeder {
            invite: invite(&sim, DeviceRole::Server).await,
            name: "again".into(),
            address: Some(sim.address.to_string()),
        });
        assert!(matches!(again.await, Err(Error::AlreadyAdopted)));
        server.service.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn settings_change_against_the_current_generation_only() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let server = Server::start().await;
        let id = adopt(&server, &sim).await;
        link(&server.service, id, Link::Online).await;
        let service = &server.service;

        let settings = service.settings(id).await.unwrap();
        let patch = SettingsPatch {
            volume: Some(30),
            ..SettingsPatch::default()
        };
        let changed = service
            .put_settings(&ada(), id, settings.generation, patch, Some([1; 16]))
            .await
            .unwrap();
        assert_eq!(changed.volume, 30);
        assert_eq!(changed.generation, Generation(settings.generation.0 + 1));

        // A retry under the same id is answered once, not applied again.
        let retried = service
            .put_settings(&ada(), id, settings.generation, patch, Some([1; 16]))
            .await
            .unwrap();
        assert_eq!(retried, changed);

        let stale = service
            .put_settings(&ada(), id, settings.generation, patch, Some([2; 16]))
            .await;
        let Err(Error::Stale(current)) = stale else {
            panic!("{stale:?}");
        };
        assert_eq!(current, changed);

        let loud = SettingsPatch {
            volume: Some(101),
            ..SettingsPatch::default()
        };
        assert!(matches!(
            service
                .put_settings(&ada(), id, changed.generation, loud, Some([3; 16]))
                .await,
            Err(Error::Invalid(Invalid::Volume))
        ));

        // The settings change's key, reused for a lock, is a new write: the
        // feeder remembers writes by id, and the id binds the kind of write.
        service
            .set_lock(&ada(), id, false, Some([1; 16]))
            .await
            .unwrap();
        // Another user's same key is their own write, against the current
        // generation.
        let theirs = service
            .put_settings(
                &user(2, "bob"),
                id,
                changed.generation,
                SettingsPatch {
                    volume: Some(40),
                    ..SettingsPatch::default()
                },
                Some([1; 16]),
            )
            .await
            .unwrap();
        assert_eq!(theirs.generation, Generation(changed.generation.0 + 1));
        server.service.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_lock_change_is_recorded_once_and_published_live() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let server = Server::start().await;
        let id = adopt(&server, &sim).await;
        link(&server.service, id, Link::Online).await;
        let service = &server.service;
        let mut live = service.hub().subscribe(None);
        assert!(matches!(live.resume, Resume::Replay(ref missed) if missed.is_empty()));

        let key: IdempotencyKey = [9; 16];
        service
            .set_lock(&ada(), id, false, Some(key))
            .await
            .unwrap();
        service
            .set_lock(&ada(), id, false, Some(key))
            .await
            .unwrap();

        let event = tokio::time::timeout(WAIT, async {
            loop {
                let stamped = live.live.recv().await.unwrap();
                if let StreamEvent::Event { feeder, event } = &stamped.event
                    && matches!(event.body, EventBody::Lock { .. })
                {
                    assert_eq!(*feeder, id);
                    return event.clone();
                }
            }
        })
        .await
        .unwrap();
        let EventBody::Lock { locked, by } = event.body else {
            unreachable!();
        };
        assert!(!locked);
        assert!(matches!(by, Source::Client { .. }));

        let locks = service
            .events(
                id,
                EventQuery {
                    kind: Some(EventKind::Lock),
                    ..EventQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(locks.events.len(), 1, "the retry did not lock twice");
        server.service.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn history_recorded_while_the_server_was_down_arrives_on_restart() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let server = Server::start().await;
        let id = adopt(&server, &sim).await;
        link(&server.service, id, Link::Online).await;
        let dir = server.stop().await;

        // The owner changes the lock twice while the server is away.
        let mut owner = open_owner(&sim).await;
        for (n, locked) in [(1u8, false), (2, true)] {
            let answer = owner.ask([n; 16], Op::SetLock { locked }).await.unwrap();
            assert_eq!(answer.outcome(), Outcome::Done { generation: None });
        }
        let answer = owner.ask([3; 16], Op::Status).await.unwrap();
        let Outcome::Status(status) = answer.outcome() else {
            panic!("{answer:?}");
        };
        let newest = status.last_event.unwrap();
        drop(owner);

        let server = Server::reopen(dir).await;
        link(&server.service, id, Link::Online).await;
        synced(&server.service, id, newest).await;
        let page = server
            .service
            .events(id, EventQuery::default())
            .await
            .unwrap();
        let seqs: Vec<u64> = page.events.iter().map(|event| event.seq).collect();
        assert!(
            seqs.is_sorted() && seqs.windows(2).all(|w| w[0] < w[1]),
            "{seqs:?}"
        );
        let locks: Vec<bool> = page
            .events
            .iter()
            .filter_map(|event| match event.body {
                EventBody::Lock { locked, .. } => Some(locked),
                _ => None,
            })
            .collect();
        assert_eq!(locks, [false, true]);
        assert!(page.gaps.is_empty());
        server.service.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_feeder_that_goes_away_is_offline_until_it_returns() {
    local(async {
        let flash = Flash::memory();
        let sim = Running::start(&flash, Some(*owner().public())).await;
        let address = sim.address;
        let server = Server::start().await;
        let id = adopt(&server, &sim).await;
        link(&server.service, id, Link::Online).await;
        let mut updates = server.service.hub().subscribe(None);

        sim.stop().await;
        link(&server.service, id, Link::Offline).await;
        assert!(matches!(
            server
                .service
                .set_lock(&ada(), id, false, Some([5; 16]))
                .await,
            Err(Error::Offline)
        ));
        assert!(matches!(
            server.service.settings(id).await,
            Err(Error::Offline)
        ));
        let detail = server.service.feeder(id).await.unwrap();
        assert!(!detail.status.unwrap().live, "the cached status, marked");

        let sim = Running::start_at(&flash, Some(*owner().public()), address).await;
        link(&server.service, id, Link::Online).await;
        server
            .service
            .set_lock(&ada(), id, false, Some([5; 16]))
            .await
            .unwrap();

        let mut seen = Vec::new();
        while let Ok(stamped) = updates.live.try_recv() {
            if let StreamEvent::Connection { connection, .. } = stamped.event {
                seen.push(connection);
            }
        }
        assert!(seen.starts_with(&[Link::Offline]), "{seen:?}");
        assert_eq!(seen.last(), Some(&Link::Online));
        server.service.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_revoked_server_key_is_refused() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let server = Server::start().await;
        let id = adopt(&server, &sim).await;
        link(&server.service, id, Link::Online).await;

        let mut owner = open_owner(&sim).await;
        let answer = owner.ask([1; 16], Op::ListClients).await.unwrap();
        let Outcome::Clients(clients) = answer.outcome() else {
            panic!("{answer:?}");
        };
        let key = clients
            .clients
            .iter()
            .flatten()
            .find(|client| client.role == DeviceRole::Server)
            .unwrap()
            .key;
        let answer = owner.ask([2; 16], Op::RevokeClient { key }).await.unwrap();
        assert_eq!(answer.outcome(), Outcome::Done { generation: None });

        link(&server.service, id, Link::Refused).await;
        assert!(matches!(
            server
                .service
                .set_lock(&ada(), id, true, Some([3; 16]))
                .await,
            Err(Error::Offline)
        ));
        server.service.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn an_invite_for_a_member_is_adopted_with_that_role_and_bad_input_is_refused() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let server = Server::start().await;
        let service = &server.service;

        let member = service
            .adopt(AdoptFeeder {
                invite: invite(&sim, DeviceRole::Member).await,
                name: "hall".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap();
        assert_eq!(member.role, ClientRole::Member);

        let used = invite(&sim, DeviceRole::Server).await;
        let request = |invite: &str, address: Option<&str>| AdoptFeeder {
            invite: invite.into(),
            name: "x".into(),
            address: address.map(Into::into),
        };
        assert!(matches!(
            service.adopt(request("nonsense", Some("127.0.0.1"))).await,
            Err(Error::Invite(_))
        ));
        assert!(matches!(
            service.adopt(request(&used, None)).await,
            Err(Error::AlreadyAdopted)
        ));
        let other = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let fresh = invite(&other, DeviceRole::Server).await;
        assert!(matches!(
            service.adopt(request(&fresh, None)).await,
            Err(Error::Invalid(Invalid::AddressRequired))
        ));
        assert!(matches!(
            service.adopt(request(&fresh, Some("not an address"))).await,
            Err(Error::Invalid(Invalid::Address))
        ));
        let query = EventQuery {
            limit: Some(0),
            ..EventQuery::default()
        };
        assert!(matches!(
            service.events(member.id, query).await,
            Err(Error::Invalid(Invalid::Limit))
        ));
        server.service.shutdown().await;
        other.stop().await;
        sim.stop().await;
    });
}

#[test]
fn the_clock_is_set_again_on_schedule_and_samples_are_kept() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("server.db")).await.unwrap();
        let mut config = config();
        config.timing.time_sync = Duration::from_millis(300);
        let service = Service::start(store, config, Arc::new(Fixed), Arc::new(SystemRandom))
            .await
            .unwrap();
        let server = Server { dir, service };
        let mut updates = server.service.hub().subscribe(None);
        let id = adopt(&server, &sim).await;

        // The connect's sync, then at least two on schedule; and the
        // simulator's power sample, every 5 s.
        let (mut syncs, mut sample) = (0, None);
        tokio::time::timeout(WAIT, async {
            while syncs < 3 || sample.is_none() {
                match &updates.live.recv().await.unwrap().event {
                    StreamEvent::Event { event, .. }
                        if matches!(
                            event.body,
                            EventBody::ClockSync {
                                by: Source::Client { .. },
                                ..
                            }
                        ) =>
                    {
                        syncs += 1;
                    }
                    StreamEvent::Sample { sample: got, .. } => sample = Some(*got),
                    _ => {}
                }
            }
        })
        .await
        .expect("three clock syncs and a sample");

        let reader = Store::open(&server.dir.path().join("server.db"))
            .await
            .unwrap();
        let sample = sample.unwrap();
        let kept = reader.samples(id, sample.value.metric(), 10).await.unwrap();
        assert!(kept.iter().any(|row| row.uptime_ms == sample.uptime_ms));
        server.service.shutdown().await;
        sim.stop().await;
    });
}

/// The feeder stalls after a write reaches it: the caller hears `Unknown`,
/// never a failure, and its retry under the same key is answered from the
/// feeder's memory, so the lock changes once.
#[test]
fn an_unanswered_write_is_unknown_and_its_retry_applies_once() {
    local(async {
        let sim = Running::start(&Flash::memory(), Some(*owner().public())).await;
        let relay = Relay::start(sim.address).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("server.db")).await.unwrap();
        let mut config = config();
        config.timing.answer = Duration::from_millis(500);
        let service = Service::start(store, config, Arc::new(Fixed), Arc::new(SystemRandom))
            .await
            .unwrap();
        let id = service
            .adopt(AdoptFeeder {
                invite: invite(&sim, DeviceRole::Server).await,
                name: "kitchen".into(),
                address: Some(relay.address.to_string()),
            })
            .await
            .unwrap()
            .id;
        link(&service, id, Link::Online).await;

        relay.pause();
        let key = Some([0x51; 16]);
        assert!(matches!(
            service.set_lock(&ada(), id, false, key).await,
            Err(Error::Unknown)
        ));
        relay.resume();
        service.set_lock(&ada(), id, false, key).await.unwrap();

        let locks = || async {
            let query = EventQuery {
                kind: Some(EventKind::Lock),
                ..EventQuery::default()
            };
            service.events(id, query).await.unwrap().events.len()
        };
        tokio::time::timeout(WAIT, async {
            while locks().await == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(locks().await, 1, "the retry did not lock twice");
        service.shutdown().await;
        sim.stop().await;
    });
}
