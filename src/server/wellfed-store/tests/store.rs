//! The store against a fresh SQLite file per test.

use jiff::Timestamp;
use wellfed_api::{
    ClientRole, EventKind, FeederId, FeederStatus, Generation, MetricKind, Role, Settings, UserId,
    WifiSource,
};
use wellfed_store::{Error, FeederRow, NewEvent, NewFeeder, NewSample, NewUser, SealedKey, Store};
use tempfile::TempDir;
use uuid::Uuid;

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("server.db")).await.unwrap();
    (dir, store)
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

fn user(name: &str) -> NewUser<'_> {
    NewUser {
        id: UserId(Uuid::now_v7()),
        name,
        password_hash: "$argon2id$hash",
        role: Role::Owner,
        created: at(1_000),
    }
}

async fn adopt(store: &Store, identity: u8) -> FeederId {
    let id = FeederId(Uuid::now_v7());
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

fn event(seq: u64, kind: EventKind) -> NewEvent<'static> {
    NewEvent {
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

    assert!(store.create_first_owner(user("ada")).await.unwrap());
    assert!(!store.create_first_owner(user("bob")).await.unwrap());

    assert!(store.users_exist().await.unwrap());
    let login = store.login("ada").await.unwrap().unwrap();
    assert_eq!(login.user.name, "ada");
    assert_eq!(login.user.role, Role::Owner);
    assert_eq!(login.password_hash, "$argon2id$hash");
    assert!(store.login("bob").await.unwrap().is_none());
}

#[tokio::test]
async fn a_session_ends_at_its_expiry_or_logout() {
    let (_dir, store) = fresh().await;
    let new = user("ada");
    let id = new.id;
    store.create_first_owner(new).await.unwrap();
    let (live, expired) = ([1; 32], [2; 32]);
    store
        .create_session(&live, id, at(0), at(10_000))
        .await
        .unwrap();
    store
        .create_session(&expired, id, at(0), at(5_000))
        .await
        .unwrap();

    let user = store.session_user(&live, at(6_000)).await.unwrap().unwrap();
    assert_eq!(user.id, id);
    assert!(
        store
            .session_user(&expired, at(6_000))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .session_user(&[3; 32], at(6_000))
            .await
            .unwrap()
            .is_none()
    );

    assert_eq!(store.delete_expired_sessions(at(6_000)).await.unwrap(), 1);
    store.delete_session(&live).await.unwrap();
    assert!(
        store
            .session_user(&live, at(6_000))
            .await
            .unwrap()
            .is_none()
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
            id: FeederId(Uuid::now_v7()),
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
    assert_eq!(store.feeder(FeederId(Uuid::now_v7())).await.unwrap(), None);
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
    assert_eq!(store.feeder(id).await.unwrap().unwrap().synced, Some(5));

    store.record_gap(id, 6, 9, at(10)).await.unwrap();
    store.record_gap(id, 6, 9, at(11)).await.unwrap();
    assert_eq!(store.feeder(id).await.unwrap().unwrap().synced, Some(9));
    assert!(
        store
            .record_event(id, event(10, EventKind::Lock))
            .await
            .unwrap()
    );

    let all = store.events(id, None, 100, None).await.unwrap();
    let seqs: Vec<u64> = all.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [3, 5, 10]);
    assert_eq!(all[0].body, [0, 1, 2]);
    assert_eq!(all[0].at, Some(at(1_700_000_000_000)));
    assert_eq!(all[0].received, at(1_700_000_000_500));

    let locks = store
        .events(id, Some(3), 100, Some(EventKind::Lock))
        .await
        .unwrap();
    assert_eq!(locks.iter().map(|e| e.seq).collect::<Vec<_>>(), [10]);
    let page = store.events(id, None, 2, None).await.unwrap();
    assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), [3, 5]);

    let gaps = store.gaps(id, Some(5), 10).await.unwrap();
    assert_eq!((gaps.len(), gaps[0].from, gaps[0].to), (1, 6, 9));
    assert_eq!(gaps[0].received, at(10));
    assert!(store.gaps(id, Some(9), 10).await.unwrap().is_empty());
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
    let store = Store::open(&path).await.unwrap();
    store.create_first_owner(user("ada")).await.unwrap();
    store.close().await;

    let store = Store::open(&path).await.unwrap();
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
/// database first, and a copy left by one attempt does not block the next.
#[tokio::test]
async fn pending_migrations_copy_the_database_first_on_every_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    Store::open(&path).await.unwrap().close().await;

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

    for attempt in 1..=2 {
        let opened = Store::open(&path).await;
        assert!(matches!(opened, Err(Error::Migrate(_))), "{opened:?}");
        assert_eq!(copies(dir.path()), attempt);
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}
