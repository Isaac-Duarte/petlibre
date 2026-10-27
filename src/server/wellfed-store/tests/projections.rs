//! The projections beside the raw events: what ingest writes equals a
//! rebuild from the raw rows, whatever the order, replays included, and a
//! new projection version rebuilds them.
//!
//! The store keeps whatever bytes it is given as the raw body; these tests
//! store the event's JSON and decode it back, standing in for the mirror's
//! postcard.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;
use wellfed_api::{
    Blocked, ClientRole, EventBody, FeedOutcome, FeedReason, FeederId, Hex, Source,
};
use wellfed_store::{
    Boot, ClockSync, Feed, LockChange, NewEvent, NewFeeder, PROJECTION_VERSION, Projected, Refresh,
    SealedKey, Store,
};
use proptest::prelude::*;
use proptest::sample::select;
use tempfile::TempDir;
use uuid::Uuid;

const DATABASE: &str = "server.db";

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join(DATABASE), at(0))
        .await
        .unwrap();
    (dir, store)
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn adopt(store: &Store, n: u8) -> FeederId {
    let id = FeederId(Uuid::from_u128(u128::from(n) + 1));
    store
        .insert_feeder(NewFeeder {
            id,
            identity: [n; 32],
            name: "kitchen",
            role: ClientRole::Server,
            address: None,
            key: SealedKey {
                nonce: [7; 12],
                sealed: vec![1, 2, 3],
            },
            created: at(2_000),
        })
        .await
        .unwrap();
    id
}

fn decode(raw: &[u8]) -> Option<EventBody> {
    serde_json::from_slice(raw).ok()
}

/// One pushed event, as the mirror hands it to the store.
#[derive(Clone, Debug)]
struct Pushed {
    feeder: usize,
    epoch: u32,
    seq: u64,
    at_s: Option<i64>,
    received_ms: i64,
    body: EventBody,
}

async fn ingest(store: &Store, feeders: &[FeederId], pushed: &Pushed) -> bool {
    let raw = serde_json::to_vec(&pushed.body).unwrap();
    store
        .record_event(
            feeders[pushed.feeder],
            NewEvent {
                epoch: pushed.epoch,
                seq: pushed.seq,
                at: pushed.at_s.map(|s| Timestamp::from_second(s).unwrap()),
                received: at(pushed.received_ms),
                body: pushed.body,
                raw: &raw,
            },
        )
        .await
        .unwrap()
}

/// Every projection of one feeder, over all time.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    feeds: Vec<Projected<Feed>>,
    locks: Vec<Projected<LockChange>>,
    boots: Vec<Projected<Boot>>,
    clocks: Vec<Projected<ClockSync>>,
    blocked: Vec<Projected<Option<Blocked>>>,
}

async fn snapshot(store: &Store, id: FeederId) -> Snapshot {
    let (from, to) = (Timestamp::MIN, Timestamp::MAX);
    Snapshot {
        feeds: store.feeds(id, from, to).await.unwrap(),
        locks: store.lock_changes(id, from, to).await.unwrap(),
        boots: store.boots(id, from, to).await.unwrap(),
        clocks: store.clock_syncs(id, from, to).await.unwrap(),
        blocked: store.blocked_changes(id, from, to).await.unwrap(),
    }
}

/// The projections worked out without the store: the raw log keeps the
/// first arrival of each event number; a feed is the body of its highest
/// numbered event, and every other kind is one row per event. Ordered as
/// the reads order them.
fn expected(events: &[Pushed], feeder: usize) -> Snapshot {
    let mut held: BTreeMap<(u32, u64), &Pushed> = BTreeMap::new();
    for event in events.iter().filter(|event| event.feeder == feeder) {
        held.entry((event.epoch, event.seq)).or_insert(event);
    }

    fn with<T>(event: &Pushed, value: T) -> Projected<T> {
        Projected {
            epoch: event.epoch,
            seq: event.seq,
            at: event.at_s.map(|s| Timestamp::from_second(s).unwrap()),
            received: at(event.received_ms),
            value,
        }
    }

    let mut latest: BTreeMap<(u32, u32), (&Pushed, Feed)> = BTreeMap::new();
    let mut snapshot = Snapshot {
        feeds: Vec::new(),
        locks: Vec::new(),
        boots: Vec::new(),
        clocks: Vec::new(),
        blocked: Vec::new(),
    };
    // In key order, so a later number replaces an earlier one.
    for event in held.values() {
        match event.body {
            EventBody::Fed {
                feed,
                requested,
                completed,
                outcome,
                reason,
                by,
            } => {
                let value = Feed {
                    feed,
                    by,
                    requested,
                    completed,
                    outcome,
                    reason,
                };
                latest.insert((event.epoch, feed), (event, value));
            }
            EventBody::Lock { locked, by } => {
                snapshot.locks.push(with(event, LockChange { locked, by }));
            }
            EventBody::Boot { bank, reset_flags } => {
                snapshot.boots.push(with(event, Boot { bank, reset_flags }));
            }
            EventBody::ClockSync { by, jump_ms } => {
                snapshot.clocks.push(with(event, ClockSync { by, jump_ms }));
            }
            EventBody::FeedingBlocked { reason } => snapshot.blocked.push(with(event, reason)),
            _ => {}
        }
    }
    snapshot.feeds = latest
        .into_values()
        .map(|(event, value)| with(event, value))
        .collect();

    fn order<T>(rows: &mut [Projected<T>]) {
        rows.sort_by_key(|row| (row.received, row.epoch, row.seq));
    }
    order(&mut snapshot.feeds);
    order(&mut snapshot.locks);
    order(&mut snapshot.boots);
    order(&mut snapshot.clocks);
    order(&mut snapshot.blocked);
    snapshot
}

/// Mark the projections as another version's and corrupt every projection
/// table (rows dropped, rows changed, rows no event made), as a change to
/// the projection code could leave them. Through a connection of its own:
/// the store has no way to do it.
async fn stale(path: &Path, version: i64) {
    let url = format!("sqlite://{}", path.display());
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let statements = [
        "DELETE FROM feeds WHERE seq % 2 = 0",
        "UPDATE feeds SET completed = completed + 7",
        "INSERT OR REPLACE INTO feeds SELECT id, 1, 999, 999, 'panel', NULL, 1, 1, 'done', 'done', NULL, 0
         FROM feeders",
        "DELETE FROM lock_changes WHERE seq % 2 = 0",
        "UPDATE lock_changes SET locked = 1 - locked",
        "INSERT OR REPLACE INTO lock_changes SELECT id, 1, 999, 1, 'feeder', NULL, NULL, 0 FROM feeders",
        "DELETE FROM boots WHERE seq % 2 = 0",
        "UPDATE boots SET bank = bank + 1",
        "INSERT OR REPLACE INTO boots SELECT id, 1, 999, 0, 0, NULL, 0 FROM feeders",
        "DELETE FROM clock_syncs WHERE seq % 2 = 0",
        "UPDATE clock_syncs SET jump_ms = 1",
        "INSERT OR REPLACE INTO clock_syncs SELECT id, 1, 999, 'feeder', NULL, NULL, NULL, 0 FROM feeders",
        "DELETE FROM blocked_changes WHERE seq % 2 = 0",
        "UPDATE blocked_changes SET reason = 'fault'",
        "INSERT OR REPLACE INTO blocked_changes SELECT id, 1, 999, NULL, NULL, 0 FROM feeders",
    ];
    sqlx::query("UPDATE projection_version SET version = ?")
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    for statement in statements {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;
}

fn source() -> impl Strategy<Value = Source> {
    prop_oneof![
        Just(Source::Panel),
        Just(Source::Feeder),
        any::<[u8; 4]>().prop_map(|client| Source::Client {
            client: Hex(client)
        }),
    ]
}

/// Bodies of every kind that projects, and one that does not. Feed numbers
/// and sequence numbers come from small ranges, so one feed often has
/// several events and one number several bodies.
fn body() -> impl Strategy<Value = EventBody> {
    prop_oneof![
        (
            0u32..4,
            0u8..4,
            0u8..4,
            select(FeedOutcome::ALL.to_vec()),
            select(FeedReason::ALL.to_vec()),
            source(),
        )
            .prop_map(|(feed, requested, completed, outcome, reason, by)| {
                EventBody::Fed {
                    feed,
                    requested,
                    completed,
                    outcome,
                    reason,
                    by,
                }
            }),
        (any::<bool>(), source()).prop_map(|(locked, by)| EventBody::Lock { locked, by }),
        (0u8..2, any::<u32>())
            .prop_map(|(bank, reset_flags)| EventBody::Boot { bank, reset_flags }),
        (source(), proptest::option::of(any::<i32>()))
            .prop_map(|(by, jump_ms)| EventBody::ClockSync { by, jump_ms }),
        proptest::option::of(select(Blocked::ALL.to_vec()))
            .prop_map(|reason| EventBody::FeedingBlocked { reason }),
        (0u16..10).prop_map(|count| EventBody::Lost { count }),
    ]
}

fn pushed() -> impl Strategy<Value = Pushed> {
    (
        0usize..2,
        1u32..3,
        1u64..16,
        proptest::option::of(1_700_000_000i64..1_700_000_100),
        0i64..50,
        body(),
    )
        .prop_map(|(feeder, epoch, seq, at_s, received_ms, body)| Pushed {
            feeder,
            epoch,
            seq,
            at_s,
            received_ms: 1_700_000_000_000 + received_ms,
            body,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Events arrive in any order, and some again (a reconnect's replay):
    /// what ingest projected equals a rebuild from the raw rows.
    #[test]
    fn ingest_projects_what_a_rebuild_does(
        events in proptest::collection::vec(pushed(), 1..40),
        replays in proptest::collection::vec(any::<proptest::sample::Index>(), 0..20),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (dir, store) = fresh().await;
            assert_eq!(
                store.refresh_projections(decode).await.unwrap(),
                Refresh::Rebuilt { events: 0, undecoded: 0 }
            );
            let feeders = [adopt(&store, 1).await, adopt(&store, 2).await];

            for event in &events {
                ingest(&store, &feeders, event).await;
            }
            for replay in &replays {
                let again = replay.get(&events);
                assert!(!ingest(&store, &feeders, again).await, "a replay is not new");
            }
            let live = [snapshot(&store, feeders[0]).await, snapshot(&store, feeders[1]).await];
            let model = [expected(&events, 0), expected(&events, 1)];
            assert_eq!(live, model, "ingest projects what the model says");

            stale(&dir.path().join(DATABASE), 0).await;
            let Refresh::Rebuilt { undecoded, .. } =
                store.refresh_projections(decode).await.unwrap()
            else {
                panic!("a stale version rebuilds");
            };
            assert_eq!(undecoded, 0);
            let rebuilt = [snapshot(&store, feeders[0]).await, snapshot(&store, feeders[1]).await];
            assert_eq!(rebuilt, model, "the rebuild projects what the model says");
            store.close().await;
        });
    }
}

/// More events than one rebuild batch (512) holds, over two feeders, so
/// the rebuild's batches cross feeders and epochs: it still projects what
/// ingest did, every row once.
#[tokio::test]
async fn a_rebuild_reads_across_its_batches() {
    let (dir, store) = fresh().await;
    let feeders = [adopt(&store, 1).await, adopt(&store, 2).await];
    let mut events = Vec::new();
    for n in 0..1_300u64 {
        let feed = u32::try_from(n % 97).unwrap();
        let body = match n % 4 {
            0 => EventBody::Fed {
                feed,
                requested: 1,
                completed: u8::try_from(n % 2).unwrap(),
                outcome: FeedOutcome::ALL[usize::try_from(n).unwrap() % 5],
                reason: FeedReason::Done,
                by: Source::Feeder,
            },
            1 => EventBody::Lock {
                locked: n % 3 == 0,
                by: Source::Panel,
            },
            2 => EventBody::Boot {
                bank: 0,
                reset_flags: u32::try_from(n).unwrap(),
            },
            _ => EventBody::FeedingBlocked { reason: None },
        };
        events.push(Pushed {
            feeder: usize::try_from(n % 2).unwrap(),
            epoch: u32::try_from(n % 3).unwrap() + 1,
            seq: n / 6 + 1,
            at_s: None,
            received_ms: i64::try_from(n).unwrap(),
            body,
        });
    }
    for event in &events {
        ingest(&store, &feeders, event).await;
    }
    let model = [expected(&events, 0), expected(&events, 1)];
    let live = [
        snapshot(&store, feeders[0]).await,
        snapshot(&store, feeders[1]).await,
    ];
    assert_eq!(live, model);

    stale(&dir.path().join(DATABASE), 0).await;
    assert_eq!(
        store.refresh_projections(decode).await.unwrap(),
        Refresh::Rebuilt {
            events: 1_300,
            undecoded: 0
        }
    );
    let rebuilt = [
        snapshot(&store, feeders[0]).await,
        snapshot(&store, feeders[1]).await,
    ];
    assert_eq!(rebuilt, model);
}

/// A feed's row is its latest event, wherever that arrives; a replay
/// changes nothing; the read window is the server's receive time.
#[tokio::test]
async fn a_feed_is_its_latest_event_and_reads_take_a_window() {
    let (_dir, store) = fresh().await;
    let feeders = [adopt(&store, 1).await];
    let fed = |seq, completed, outcome, received_ms| Pushed {
        feeder: 0,
        epoch: 3,
        seq,
        at_s: Some(1_700_000_000),
        received_ms,
        body: EventBody::Fed {
            feed: 9,
            requested: 2,
            completed,
            outcome,
            reason: FeedReason::Done,
            by: Source::Client {
                client: Hex([1, 2, 3, 4]),
            },
        },
    };

    assert!(ingest(&store, &feeders, &fed(5, 2, FeedOutcome::Done, 2_000)).await);
    assert!(ingest(&store, &feeders, &fed(4, 0, FeedOutcome::Uncertain, 3_000)).await);
    assert!(!ingest(&store, &feeders, &fed(5, 1, FeedOutcome::Failed, 4_000)).await);

    let feeds = store.feeds(feeders[0], at(0), at(10_000)).await.unwrap();
    assert_eq!(feeds.len(), 1, "one row per feed");
    let feed = feeds[0];
    assert_eq!((feed.epoch, feed.seq, feed.received), (3, 5, at(2_000)));
    assert_eq!(
        feed.at,
        Some(Timestamp::from_second(1_700_000_000).unwrap())
    );
    assert_eq!(
        feed.value,
        Feed {
            feed: 9,
            by: Source::Client {
                client: Hex([1, 2, 3, 4])
            },
            requested: 2,
            completed: 2,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
        }
    );

    assert!(
        store
            .feeds(feeders[0], at(0), at(2_000))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .feeds(feeders[0], at(2_000), at(2_001))
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Projections built by another version are rebuilt at the next refresh,
/// and ones built by this version are left as they are.
#[tokio::test]
async fn a_new_projection_version_rebuilds_from_the_raw_events() {
    let (dir, store) = fresh().await;
    let path = dir.path().join(DATABASE);
    store.refresh_projections(decode).await.unwrap();
    let feeders = [adopt(&store, 1).await];
    let fed = Pushed {
        feeder: 0,
        epoch: 1,
        seq: 1,
        at_s: None,
        received_ms: 1_000,
        body: EventBody::Fed {
            feed: 1,
            requested: 1,
            completed: 1,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
            by: Source::Panel,
        },
    };
    ingest(&store, &feeders, &fed).await;
    let all = |store: Store| async move {
        store
            .feeds(feeders[0], Timestamp::MIN, Timestamp::MAX)
            .await
            .unwrap()
    };
    let before = all(store.clone()).await;
    assert_eq!(before.len(), 1);

    stale(&path, i64::from(PROJECTION_VERSION)).await;
    assert_eq!(
        store.refresh_projections(decode).await.unwrap(),
        Refresh::Current
    );
    assert_ne!(
        all(store.clone()).await,
        before,
        "the same version is kept, corrupt rows and all"
    );

    stale(&path, i64::from(PROJECTION_VERSION) - 1).await;
    assert_eq!(
        store.refresh_projections(decode).await.unwrap(),
        Refresh::Rebuilt {
            events: 1,
            undecoded: 0
        }
    );
    assert_eq!(all(store.clone()).await, before);
    assert_eq!(
        store.refresh_projections(decode).await.unwrap(),
        Refresh::Current
    );
}

/// A stored body that no longer decodes is counted and projects nothing;
/// the rest still rebuild.
#[tokio::test]
async fn a_body_that_does_not_decode_is_counted_and_skipped() {
    let (dir, store) = fresh().await;
    let feeders = [adopt(&store, 1).await];
    let lock = |seq, locked| Pushed {
        feeder: 0,
        epoch: 1,
        seq,
        at_s: None,
        received_ms: 1_000,
        body: EventBody::Lock {
            locked,
            by: Source::Feeder,
        },
    };
    ingest(&store, &feeders, &lock(1, true)).await;
    ingest(&store, &feeders, &lock(2, false)).await;
    store.refresh_projections(decode).await.unwrap();

    stale(&dir.path().join(DATABASE), 0).await;
    let only_unlocks = |raw: &[u8]| decode(raw).filter(|body| *body == lock(2, false).body);
    assert_eq!(
        store.refresh_projections(only_unlocks).await.unwrap(),
        Refresh::Rebuilt {
            events: 2,
            undecoded: 1
        }
    );
    let locks = store
        .lock_changes(feeders[0], Timestamp::MIN, Timestamp::MAX)
        .await
        .unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!((locks[0].seq, locks[0].value.locked), (2, false));
}
