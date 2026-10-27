//! The store against a fresh SQLite file per test.

use std::sync::atomic::{AtomicU64, Ordering};

use jiff::Timestamp;
use wellfed_api::{
    Blocked, ClientRole, EventKind, FeederId, FeederStatus, Generation, Household, MetricKind,
    Readiness, Role, Settings, TimeFormat, Units, UserId, WifiSource,
};
use wellfed_store::{
    Change, Error, FeederRow, NewEvent, NewFeeder, NewSample, NewUser, SealedKey, Store, Synced,
};
use tempfile::TempDir;
use uuid::Uuid;

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("server.db"), at(0))
        .await
        .unwrap();
    (dir, store)
}

/// A new id for each call: the store doesn't care how ids are made, and a
/// test reads no clock.
fn fresh_id() -> Uuid {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Uuid::from_u128(u128::from(NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

fn user(name: &str) -> NewUser<'_> {
    NewUser {
        id: UserId(fresh_id()),
        name,
        password_hash: "$argon2id$hash",
        role: Role::Owner,
        expires: None,
        created: at(1_000),
    }
}

fn household() -> Household {
    Household {
        name: "Home".into(),
        units: Units::Grams,
        time_format: TimeFormat::H24,
        zone: "Europe/Berlin".into(),
    }
}

async fn adopt(store: &Store, identity: u8) -> FeederId {
    let id = FeederId(fresh_id());
    store
        .insert_feeder(NewFeeder {
            id,
            identity: [identity; 32],
            name: "kitchen",
            role: ClientRole::Server,
            address: Some("192.0.2.7:4747"),
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

/// The feeder's ring epoch in most tests.
const EPOCH: u32 = 7;

fn event(seq: u64, kind: EventKind) -> NewEvent<'static> {
    NewEvent {
        epoch: EPOCH,
        seq,
        at: Some(at(1_700_000_000_000)),
        received: at(1_700_000_000_500),
        kind,
        body: &[0, 1, 2],
    }
}

#[tokio::test]
async fn only_the_first_owner_is_created_by_setup() {
    let (_dir, store) = fresh().await;
    assert!(!store.users_exist().await.unwrap());

    assert!(
        store
            .create_first_owner(user("ada"), &household())
            .await
            .unwrap()
    );
    assert!(
        !store
            .create_first_owner(user("bob"), &household())
            .await
            .unwrap()
    );

    assert!(store.users_exist().await.unwrap());
    let login = store.login("ada").await.unwrap().unwrap();
    assert_eq!(login.user.name, "ada");
    assert_eq!(login.user.role, Role::Owner);
    assert_eq!(login.password_hash, "$argon2id$hash");
    assert!(store.login("bob").await.unwrap().is_none());
}

#[tokio::test]
async fn a_session_is_used_idles_out_or_logs_out() {
    let (_dir, store) = fresh().await;
    let new = user("ada");
    let id = new.id;
    store.create_first_owner(new, &household()).await.unwrap();
    let (live, idle) = ([1; 32], [2; 32]);
    store.create_session(&live, id, at(5_000)).await.unwrap();
    store.create_session(&idle, id, at(0)).await.unwrap();

    let session = store.session(&live).await.unwrap().unwrap();
    assert_eq!((session.user.id, session.last_used), (id, at(5_000)));
    assert!(store.session(&[3; 32]).await.unwrap().is_none());
    store.touch_session(&live, at(9_000)).await.unwrap();
    store.touch_session(&live, at(8_000)).await.unwrap();
    let session = store.session(&live).await.unwrap().unwrap();
    assert_eq!(session.last_used, at(9_000), "a use never moves it back");

    assert_eq!(store.delete_idle_sessions(at(1_000)).await.unwrap(), 1);
    assert!(store.session(&idle).await.unwrap().is_none());
    store.delete_session(&live).await.unwrap();
    assert!(store.session(&live).await.unwrap().is_none());
}

#[tokio::test]
async fn setup_writes_the_household_with_the_owner() {
    let (_dir, store) = fresh().await;
    assert_eq!(store.household().await.unwrap(), None);

    store
        .create_first_owner(user("ada"), &household())
        .await
        .unwrap();
    assert_eq!(store.household().await.unwrap(), Some(household()));

    let changed = Household {
        units: Units::Ounces,
        time_format: TimeFormat::H12,
        zone: "America/Denver".into(),
        ..household()
    };
    store.set_household(&changed).await.unwrap();
    assert_eq!(store.household().await.unwrap(), Some(changed));
}

#[tokio::test]
async fn users_are_listed_changed_and_deleted_but_never_the_last_owner() {
    let (_dir, store) = fresh().await;
    let ada = user("ada");
    let ada_id = ada.id;
    store.create_first_owner(ada, &household()).await.unwrap();
    let bob = NewUser {
        role: Role::Member,
        expires: Some(at(50_000)),
        ..user("bob")
    };
    let bob_id = bob.id;
    store.create_user(bob).await.unwrap();
    assert!(matches!(
        store.create_user(user("bob")).await,
        Err(Error::Duplicate("user"))
    ));

    let users = store.users().await.unwrap();
    let names: Vec<_> = users.iter().map(|u| (u.name.as_str(), u.role)).collect();
    assert_eq!(names, [("ada", Role::Owner), ("bob", Role::Member)]);
    assert_eq!(users[1].expires, Some(at(50_000)));

    // The only owner can be neither demoted nor deleted.
    assert_eq!(
        store.update_user(ada_id, Role::Member, None).await.unwrap(),
        Change::LastOwner
    );
    assert_eq!(store.delete_user(ada_id).await.unwrap(), Change::LastOwner);
    assert_eq!(
        store.update_user(ada_id, Role::Owner, None).await.unwrap(),
        Change::Done,
        "an owner staying one is no change of owners"
    );

    // With a second owner, either may go.
    assert_eq!(
        store.update_user(bob_id, Role::Owner, None).await.unwrap(),
        Change::Done
    );
    assert_eq!(
        store.update_user(ada_id, Role::Viewer, None).await.unwrap(),
        Change::Done
    );
    assert_eq!(
        store.user(ada_id).await.unwrap().unwrap().role,
        Role::Viewer
    );

    store.create_session(&[4; 32], ada_id, at(0)).await.unwrap();
    assert!(store.set_password(ada_id, "$argon2id$new").await.unwrap());
    assert_eq!(
        store.login("ada").await.unwrap().unwrap().password_hash,
        "$argon2id$new"
    );
    assert_eq!(store.delete_user(ada_id).await.unwrap(), Change::Done);
    assert!(
        store.session(&[4; 32]).await.unwrap().is_none(),
        "their sessions went with them"
    );
    assert_eq!(store.delete_user(ada_id).await.unwrap(), Change::NotFound);
    assert_eq!(
        store.update_user(ada_id, Role::Member, None).await.unwrap(),
        Change::NotFound
    );
    assert!(!store.set_password(ada_id, "x").await.unwrap());
}

#[tokio::test]
async fn ending_a_users_sessions_leaves_the_others() {
    let (_dir, store) = fresh().await;
    let ada = user("ada");
    let ada_id = ada.id;
    store.create_first_owner(ada, &household()).await.unwrap();
    let bob = user("bob");
    let bob_id = bob.id;
    store.create_user(bob).await.unwrap();
    for (token, id) in [([1; 32], ada_id), ([2; 32], ada_id), ([3; 32], bob_id)] {
        store.create_session(&token, id, at(0)).await.unwrap();
    }

    assert_eq!(
        store
            .delete_sessions_of(ada_id, Some(&[2; 32]))
            .await
            .unwrap(),
        1,
        "all but the one kept"
    );
    assert!(store.session(&[1; 32]).await.unwrap().is_none());
    assert!(store.session(&[2; 32]).await.unwrap().is_some());
    assert_eq!(store.delete_sessions_of(ada_id, None).await.unwrap(), 1);
    assert!(
        store.session(&[3; 32]).await.unwrap().is_some(),
        "bob's stays"
    );
    assert_eq!(
        store.password_hash(bob_id).await.unwrap().as_deref(),
        Some("$argon2id$hash")
    );
}

#[tokio::test]
async fn a_feeder_is_adopted_once_and_reads_back_whole() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 0xaa).await;

    assert!(store.feeder_adopted(&[0xaa; 32]).await.unwrap());
    assert!(!store.feeder_adopted(&[0xbb; 32]).await.unwrap());
    let again = store
        .insert_feeder(NewFeeder {
            id: FeederId(fresh_id()),
            identity: [0xaa; 32],
            name: "again",
            role: ClientRole::Member,
            address: None,
            key: SealedKey {
                nonce: [0; 12],
                sealed: vec![],
            },
            created: at(3_000),
        })
        .await;
    assert!(matches!(again, Err(Error::Duplicate("feeder"))));

    store.set_address(id, "192.0.2.8:4747").await.unwrap();
    let want = FeederRow {
        id,
        identity: [0xaa; 32],
        name: "kitchen".into(),
        role: ClientRole::Server,
        address: Some("192.0.2.8:4747".into()),
        key: SealedKey {
            nonce: [7; 12],
            sealed: vec![1, 2, 3],
        },
        synced: None,
    };
    assert_eq!(store.feeder(id).await.unwrap(), Some(want.clone()));
    assert_eq!(store.feeders().await.unwrap(), [want]);
    assert_eq!(store.feeder(FeederId(fresh_id())).await.unwrap(), None);
}

#[tokio::test]
async fn events_are_idempotent_and_the_cursor_only_moves_forward() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 1).await;

    assert!(
        store
            .record_event(id, event(5, EventKind::Boot))
            .await
            .unwrap()
    );
    assert!(
        !store
            .record_event(id, event(5, EventKind::Boot))
            .await
            .unwrap()
    );
    assert!(
        store
            .record_event(id, event(3, EventKind::Lock))
            .await
            .unwrap()
    );
    let synced = |seq| Some(Synced { epoch: EPOCH, seq });
    assert_eq!(store.feeder(id).await.unwrap().unwrap().synced, synced(5));

    store.record_gap(id, EPOCH, 6, 9, at(10)).await.unwrap();
    store.record_gap(id, EPOCH, 6, 9, at(11)).await.unwrap();
    assert_eq!(store.feeder(id).await.unwrap().unwrap().synced, synced(9));
    assert!(
        store
            .record_event(id, event(10, EventKind::Lock))
            .await
            .unwrap()
    );

    let all = store.events(id, EPOCH, None, 100, None).await.unwrap();
    let seqs: Vec<u64> = all.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [3, 5, 10]);
    assert_eq!(all[0].body, [0, 1, 2]);
    assert_eq!(all[0].at, Some(at(1_700_000_000_000)));
    assert_eq!(all[0].received, at(1_700_000_000_500));

    let locks = store
        .events(id, EPOCH, Some(3), 100, Some(EventKind::Lock))
        .await
        .unwrap();
    assert_eq!(locks.iter().map(|e| e.seq).collect::<Vec<_>>(), [10]);
    let page = store.events(id, EPOCH, None, 2, None).await.unwrap();
    assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), [3, 5]);

    let gaps = store.gaps(id, EPOCH, Some(5), 10).await.unwrap();
    assert_eq!((gaps.len(), gaps[0].from, gaps[0].to), (1, 6, 9));
    assert_eq!(gaps[0].received, at(10));
    assert!(store.gaps(id, EPOCH, Some(9), 10).await.unwrap().is_empty());
}

/// A ring re-created on the feeder counts from 1 again under a new epoch:
/// its events are new ones, the cursor follows them, and the old history
/// stays readable under its own epoch.
#[tokio::test]
async fn a_new_epoch_keeps_its_own_history_and_restarts_the_cursor() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 1).await;
    for seq in 1..=3 {
        store
            .record_event(id, event(seq, EventKind::Boot))
            .await
            .unwrap();
    }

    let mut again = event(1, EventKind::Lock);
    again.epoch = EPOCH + 1;
    assert!(store.record_event(id, again).await.unwrap(), "a new event");
    let synced = store.feeder(id).await.unwrap().unwrap().synced;
    assert_eq!(
        synced,
        Some(Synced {
            epoch: EPOCH + 1,
            seq: 1
        })
    );

    let old = store.events(id, EPOCH, None, 100, None).await.unwrap();
    let new = store.events(id, EPOCH + 1, None, 100, None).await.unwrap();
    assert_eq!((old.len(), new.len()), (3, 1));
    assert_eq!(new[0].kind, EventKind::Lock);
}

#[tokio::test]
async fn the_cached_status_and_settings_read_back() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 2).await;
    assert_eq!(store.state(id).await.unwrap(), Default::default());

    let settings = Settings {
        generation: Generation(4),
        lights: true,
        sound: false,
        volume: 60,
        writable: true,
    };
    store.record_settings(id, &settings, at(100)).await.unwrap();
    let status = FeederStatus {
        build: "feeder-sim".into(),
        bank: 1,
        uptime_ms: 5,
        boot_count: 2,
        reset_flags: 0,
        faults: 0,
        rssi: -48,
        wifi: WifiSource::Stored,
        link_losses: 0,
        dns_losses: 0,
        clock: Some(at(1_700_000_000_000)),
        power: None,
        locked: Some(true),
        settings_generation: Generation(4),
        wifi_generation: Generation(0),
        last_event: Some(9),
        sessions: 1,
        readiness: Some(Readiness::Blocked(Blocked::Joining)),
    };
    store.record_status(id, &status, at(200)).await.unwrap();

    let state = store.state(id).await.unwrap();
    assert_eq!(state.settings, Some((settings, at(100))));
    assert_eq!(state.status, Some((status, at(200))));
}

#[tokio::test]
async fn samples_are_kept_raw_newest_first() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 3).await;
    for (ms, metric, value) in [
        (1, MetricKind::Power, [1u8].as_slice()),
        (2, MetricKind::Rssi, &[2]),
        (3, MetricKind::Power, &[3]),
    ] {
        store
            .record_sample(
                id,
                NewSample {
                    received: at(ms),
                    uptime_ms: 10,
                    metric,
                    value,
                },
            )
            .await
            .unwrap();
    }

    let power = store.samples(id, MetricKind::Power, 10).await.unwrap();
    let values: Vec<&[u8]> = power.iter().map(|s| s.value.as_slice()).collect();
    assert_eq!(values, [[3u8].as_slice(), &[1]]);
    assert_eq!(
        store.samples(id, MetricKind::Rssi, 10).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn reopening_keeps_the_data_and_runs_no_migration_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let store = Store::open(&path, at(0)).await.unwrap();
    store
        .create_first_owner(user("ada"), &household())
        .await
        .unwrap();
    store.close().await;

    let store = Store::open(&path, at(0)).await.unwrap();
    assert!(store.users_exist().await.unwrap());
    assert_eq!(
        copies(dir.path()),
        0,
        "nothing was pending, so nothing was copied"
    );
}

fn copies(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name.to_string_lossy().contains(".before-")
        })
        .count()
}

/// A database whose record says a migration ran that this build lacks has
/// its own migrations pending and fails them. Each attempt copies the
/// database first, named for the time the caller gives, and a copy left by
/// one attempt does not block the next.
#[tokio::test]
async fn pending_migrations_copy_the_database_first_on_every_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    Store::open(&path, at(0)).await.unwrap().close().await;

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (0, 'from another build', 1, x'00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // The caller's clock names each copy; two attempts are two times.
    for (attempt, ms) in [(1, 1_790_000_000_001), (2, 1_790_000_000_002)] {
        let now = at(ms);
        let opened = Store::open(&path, now).await;
        assert!(matches!(opened, Err(Error::Migrate(_))), "{opened:?}");
        assert_eq!(copies(dir.path()), attempt);
        // Named for the newest pending migration and the time given.
        let newest = std::fs::read_dir(env!("CARGO_MANIFEST_DIR").to_owned() + "/migrations")
            .unwrap()
            .count();
        let copy = format!("server.db.before-{newest}-{}", now.as_millisecond());
        assert!(dir.path().join(copy).exists(), "named for the time given");
    }
}
