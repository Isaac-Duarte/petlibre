//! Samples, their numbers, the hourly rollups and raw retention.

use jiff::Timestamp;
use wellfed_api::{ClientRole, FeederId, MetricKind, SampleValue};
use wellfed_store::{
    HourRow, NewFeeder, NewSample, SampleKey, SampleRefresh, SealedKey, Span, Store,
};
use tempfile::TempDir;
use uuid::Uuid;

const HOUR: i64 = 3_600_000;

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("server.db"), at(0))
        .await
        .unwrap();
    (dir, store)
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn adopt(store: &Store, n: u8) -> FeederId {
    let id = FeederId(Uuid::from_u128(u128::from(n)));
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
            created: at(0),
        })
        .await
        .unwrap();
    id
}

/// A signal sample of `dbm` received at `ms`. The store keeps the bytes as
/// given; these carry the reading, for the refresh's stand-in decoder.
fn rssi(ms: i64, dbm: i8) -> NewSample {
    NewSample {
        received: at(ms),
        uptime_ms: 1_000,
        value: SampleValue::Rssi {
            dbm: i32::from(dbm),
        },
        raw: vec![1, dbm.to_le_bytes()[0]],
    }
}

/// A power sample with no battery: it has no number.
fn mains(ms: i64) -> NewSample {
    NewSample {
        received: at(ms),
        uptime_ms: 1_000,
        value: SampleValue::Power {
            adapter: true,
            battery_mv: None,
        },
        raw: vec![2],
    }
}

/// The stand-in decoder of [`rssi`]'s and [`mains`]' bytes.
fn decode(raw: &[u8]) -> Option<SampleValue> {
    match raw {
        [1, dbm] => Some(SampleValue::Rssi {
            dbm: i32::from(i8::from_le_bytes([*dbm])),
        }),
        [2] => Some(SampleValue::Power {
            adapter: true,
            battery_mv: None,
        }),
        _ => None,
    }
}

async fn hours(store: &Store, id: FeederId, span: Span) -> Vec<HourRow> {
    store
        .sample_hours(id, MetricKind::Rssi, span, None, 1000)
        .await
        .unwrap()
}

fn hour(start: i64, values: &[i64]) -> HourRow {
    HourRow {
        hour: at(start),
        count: u32::try_from(values.len()).unwrap(),
        min: *values.iter().min().unwrap(),
        max: *values.iter().max().unwrap(),
        sum: values.iter().sum(),
    }
}

#[tokio::test]
async fn samples_carry_their_number_and_page_in_receive_order() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 1).await;
    store
        .record_samples(
            id,
            &[rssi(30, -60), mains(10), rssi(10, -50), rssi(20, -55)],
        )
        .await
        .unwrap();

    let read = |span, after| store.samples(id, MetricKind::Rssi, span, after, 2);
    let first = read(Span::default(), None).await.unwrap();
    let numbers: Vec<_> = first.iter().map(|row| row.number).collect();
    assert_eq!(
        numbers,
        [Some(-50), Some(-55)],
        "oldest first, by receive time"
    );
    let rest = read(Span::default(), Some(first[1].key)).await.unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(
        (rest[0].number, rest[0].value.as_slice()),
        (Some(-60), [1, 196].as_slice())
    );

    let span = Span {
        from: Some(at(20)),
        to: Some(at(30)),
    };
    let inside = read(span, None).await.unwrap();
    assert_eq!(inside.len(), 1, "from is in, to is out");
    assert_eq!(inside[0].received, at(20));
    let power = store
        .samples(id, MetricKind::Power, Span::default(), None, 10)
        .await
        .unwrap();
    assert_eq!(power[0].number, None, "no battery, no number");
}

#[tokio::test]
async fn samples_received_at_the_same_time_page_by_their_order() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 1).await;
    store
        .record_samples(id, &[rssi(5, -1), rssi(5, -2), rssi(5, -3)])
        .await
        .unwrap();

    let mut after: Option<SampleKey> = None;
    let mut seen = Vec::new();
    loop {
        let page = store
            .samples(id, MetricKind::Rssi, Span::default(), after, 1)
            .await
            .unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.key);
        seen.push(last.number);
    }
    assert_eq!(seen, [Some(-1), Some(-2), Some(-3)]);
}

#[tokio::test]
async fn rollups_fold_each_sample_once_however_late_it_comes() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 1).await;
    store
        .record_samples(
            id,
            &[rssi(HOUR + 1, -50), rssi(HOUR + 2, -70), mains(HOUR + 3)],
        )
        .await
        .unwrap();
    let before = hours(&store, id, Span::default()).await;
    assert_eq!(
        before,
        [hour(HOUR, &[-50, -70])],
        "read from raw before a roll-up"
    );

    assert_eq!(store.roll_up_samples().await.unwrap(), 3);
    assert_eq!(hours(&store, id, Span::default()).await, before);
    assert_eq!(store.roll_up_samples().await.unwrap(), 0, "nothing new");

    // A sample for an hour already rolled up, then one for the next.
    store
        .record_samples(id, &[rssi(HOUR + 9, -40), rssi(2 * HOUR, -65)])
        .await
        .unwrap();
    let want = [hour(HOUR, &[-50, -70, -40]), hour(2 * HOUR, &[-65])];
    assert_eq!(
        hours(&store, id, Span::default()).await,
        want,
        "rollups and raw combined"
    );
    store.roll_up_samples().await.unwrap();
    assert_eq!(hours(&store, id, Span::default()).await, want);

    let second = Span {
        from: Some(at(HOUR + 1)),
        to: None,
    };
    assert_eq!(
        hours(&store, id, second).await,
        [hour(2 * HOUR, &[-65])],
        "an hour is in when it starts in the span"
    );
    let after = store
        .sample_hours(id, MetricKind::Rssi, Span::default(), Some(at(HOUR)), 10)
        .await
        .unwrap();
    assert_eq!(after, [hour(2 * HOUR, &[-65])]);
    let other = adopt(&store, 2).await;
    assert!(hours(&store, other, Span::default()).await.is_empty());
}

#[tokio::test]
async fn retention_drops_whole_rolled_up_hours_and_keeps_their_rollups() {
    let (_dir, store) = fresh().await;
    let id = adopt(&store, 1).await;
    store
        .record_samples(
            id,
            &[rssi(10, -50), rssi(HOUR + 10, -60), rssi(2 * HOUR, -70)],
        )
        .await
        .unwrap();

    assert_eq!(
        store.delete_samples_before(at(3 * HOUR)).await.unwrap(),
        0,
        "nothing goes before it is rolled up"
    );
    store.roll_up_samples().await.unwrap();
    // The cutoff falls inside the second hour: only the first goes.
    assert_eq!(store.delete_samples_before(at(HOUR + 20)).await.unwrap(), 1);
    let raw = store
        .samples(id, MetricKind::Rssi, Span::default(), None, 10)
        .await
        .unwrap();
    assert_eq!(raw.len(), 2);
    let want = [hour(0, &[-50]), hour(HOUR, &[-60]), hour(2 * HOUR, &[-70])];
    assert_eq!(hours(&store, id, Span::default()).await, want);

    // Emptied, the table numbers on: the rollups' mark stays right.
    assert_eq!(store.delete_samples_before(at(10 * HOUR)).await.unwrap(), 2);
    store
        .record_samples(id, &[rssi(11 * HOUR, -80)])
        .await
        .unwrap();
    let mut want = want.to_vec();
    want.push(hour(11 * HOUR, &[-80]));
    assert_eq!(hours(&store, id, Span::default()).await, want);
    assert_eq!(store.roll_up_samples().await.unwrap(), 1);
    assert_eq!(hours(&store, id, Span::default()).await, want);
}

#[tokio::test]
async fn a_refresh_numbers_the_samples_again_and_rebuilds_the_hours_they_cover() {
    let (dir, store) = fresh().await;
    assert_eq!(
        store.refresh_samples(decode).await.unwrap(),
        SampleRefresh::Decoded {
            samples: 0,
            undecoded: 0
        },
        "a new database is numbered at its first start"
    );
    let id = adopt(&store, 1).await;
    store
        .record_samples(
            id,
            &[rssi(10, -50), rssi(HOUR + 10, -60), garbled(HOUR + 20)],
        )
        .await
        .unwrap();
    store.roll_up_samples().await.unwrap();
    store.delete_samples_before(at(HOUR)).await.unwrap();
    assert_eq!(
        store.refresh_samples(decode).await.unwrap(),
        SampleRefresh::Current
    );

    // As a database from before numbers, or from another version of
    // them: the numbers are wrong and the version is not this build's.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        dir.path().join("server.db").display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE samples SET number = 7")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sample_marks SET version = 0")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let refresh = store.refresh_samples(decode).await.unwrap();
    assert_eq!(
        refresh,
        SampleRefresh::Decoded {
            samples: 2,
            undecoded: 1
        }
    );
    assert_eq!(
        store.refresh_samples(decode).await.unwrap(),
        SampleRefresh::Current
    );
    store.roll_up_samples().await.unwrap();
    let want = [hour(0, &[-50]), hour(HOUR, &[-60])];
    assert_eq!(
        hours(&store, id, Span::default()).await,
        want,
        "the hour retention emptied keeps its rollup; the other is rebuilt"
    );
    let raw = store
        .samples(id, MetricKind::Rssi, Span::default(), None, 10)
        .await
        .unwrap();
    let numbers: Vec<_> = raw.iter().map(|row| row.number).collect();
    assert_eq!(
        numbers,
        [Some(-60), None],
        "bytes that do not decode have none"
    );
}

/// A signal sample whose bytes the stand-in decoder refuses.
fn garbled(ms: i64) -> NewSample {
    NewSample {
        raw: vec![9, 9, 9],
        ..rssi(ms, -1)
    }
}

#[tokio::test]
async fn a_backup_is_the_database_as_it_stood() {
    let (dir, store) = fresh().await;
    let id = adopt(&store, 1).await;
    store.record_samples(id, &[rssi(10, -50)]).await.unwrap();
    let target = dir.path().join("copy.db");

    store.backup_into(&target).await.unwrap();
    store.record_samples(id, &[rssi(20, -60)]).await.unwrap();
    assert!(
        store.backup_into(&target).await.is_err(),
        "a backup never overwrites"
    );

    let copy = Store::open(&target, at(0)).await.unwrap();
    let rows = copy
        .samples(id, MetricKind::Rssi, Span::default(), None, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(copy.feeder(id).await.unwrap().is_some());
}

/// The migration that gave samples their numbers.
const NUMBERED: i64 = 9;

#[tokio::test]
async fn samples_from_before_numbers_are_migrated_numbered_and_rolled_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");

    // A database as a server before the samples migration left it: raw
    // samples with no id and no number, stored out of receive order.
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::migrate!().run_to(NUMBERED - 1, &pool).await.unwrap();
    let id = FeederId(Uuid::from_u128(1));
    sqlx::query(
        "INSERT INTO feeders (id, identity, name, role, key_nonce, key_sealed, created_ms)
         VALUES (?, ?, 'kitchen', 'server', x'00', x'00', 0)",
    )
    .bind(id.0)
    .bind([1u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();
    for (ms, metric, raw) in [
        (HOUR + 20, "rssi", vec![1, (-60i8).to_le_bytes()[0]]),
        (10, "rssi", vec![1, (-50i8).to_le_bytes()[0]]),
        (HOUR + 30, "power", vec![2]),
        (HOUR + 40, "rssi", vec![9, 9]),
    ] {
        sqlx::query(
            "INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, value)
             VALUES (?, ?, 1000, ?, ?)",
        )
        .bind(id.0)
        .bind(ms)
        .bind(metric)
        .bind(raw)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;

    let store = Store::open(&path, at(0)).await.unwrap();
    let refresh = store.refresh_samples(decode).await.unwrap();
    assert_eq!(
        refresh,
        SampleRefresh::Decoded {
            samples: 4,
            undecoded: 1
        }
    );
    assert_eq!(store.roll_up_samples().await.unwrap(), 4);

    let raw = store
        .samples(id, MetricKind::Rssi, Span::default(), None, 10)
        .await
        .unwrap();
    let rows: Vec<_> = raw
        .iter()
        .map(|row| (row.received, row.number, row.value.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            (at(10), Some(-50), vec![1, 206]),
            (at(HOUR + 20), Some(-60), vec![1, 196]),
            (at(HOUR + 40), None, vec![9, 9]),
        ],
        "every row kept, in receive order, numbered from its bytes"
    );
    assert!(
        raw.windows(2).all(|pair| pair[0].key.id < pair[1].key.id),
        "ids follow receive order"
    );
    let power = store
        .samples(id, MetricKind::Power, Span::default(), None, 10)
        .await
        .unwrap();
    assert_eq!(power.len(), 1);
    assert_eq!(power[0].number, None, "no battery, no number");
    assert_eq!(
        hours(&store, id, Span::default()).await,
        [hour(0, &[-50]), hour(HOUR, &[-60])]
    );
}
