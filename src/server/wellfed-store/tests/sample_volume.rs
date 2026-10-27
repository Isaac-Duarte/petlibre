//! A month of five feeders' samples at the simulator's rates (power every
//! 5 s, signal every 30 s: about 3 million rows) reads quickly at either
//! resolution. The rows are written by SQL, since only the reads are
//! measured. Writing them takes about 25 s, so the test is ignored in the
//! gate; the figures it printed are on the server board. Run it with
//! `cargo nextest run -p wellfed-store --test sample_volume --run-ignored
//! only --no-capture`.

use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{ClientRole, FeederId, MetricKind};
use wellfed_store::{NewFeeder, SealedKey, Span, Store};
use tokio::time::Instant;
use uuid::Uuid;

const FEEDERS: u8 = 5;
const DAYS: i64 = 30;
const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
const START: i64 = 1_790_000_000_000 / DAY * DAY;

/// The most any one read may take. The reads below take milliseconds; the
/// bound leaves room for a loaded machine and an unoptimized SQLite.
const QUICK: Duration = Duration::from_secs(2);

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn timed<T>(what: &str, work: impl Future<Output = T>) -> T {
    let start = Instant::now();
    let out = work.await;
    let took = start.elapsed();
    println!("{what}: {took:?}");
    assert!(took < QUICK, "{what} took {took:?}");
    out
}

#[tokio::test]
#[ignore = "about 25 s of writes; run by hand, see the module docs"]
async fn a_month_of_five_feeders_reads_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let store = Store::open(&path, at(0)).await.unwrap();
    let mut feeders = Vec::new();
    for n in 1..=FEEDERS {
        let id = FeederId(Uuid::from_u128(u128::from(n)));
        store
            .insert_feeder(NewFeeder {
                id,
                identity: [n; 32],
                name: "feeder",
                role: ClientRole::Server,
                address: None,
                key: SealedKey {
                    nonce: [0; 12],
                    sealed: vec![0],
                },
                created: at(START),
            })
            .await
            .unwrap();
        feeders.push(id);
    }

    // Every feeder's samples in the order they would arrive.
    let written = Instant::now();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    for (metric, every, base) in [("power", 5_000, 3_700), ("rssi", 30_000, -70)] {
        sqlx::query(
            "WITH RECURSIVE t(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM t WHERE i < ?1)
             INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, number, value)
             SELECT f.id, ?2 + i * ?3, i * ?3 % 4294967296, ?4, ?5 + i % 37, x'00'
             FROM t CROSS JOIN feeders AS f ORDER BY i",
        )
        .bind(DAYS * DAY / every - 1)
        .bind(START)
        .bind(every)
        .bind(metric)
        .bind(base)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
    let rows =
        usize::from(FEEDERS) * usize::try_from(DAYS * DAY / 5_000 + DAYS * DAY / 30_000).unwrap();
    println!("{rows} samples written in {:?}", written.elapsed());

    let rolled = Instant::now();
    let folded = store.roll_up_samples().await.unwrap();
    assert_eq!(folded, u64::try_from(rows).unwrap());
    println!("rolled up in {:?}", rolled.elapsed());

    // A month of hours, for each feeder.
    for &id in &feeders {
        let hours = timed("a month of power by the hour", async {
            store
                .sample_hours(id, MetricKind::Power, Span::default(), None, 1000)
                .await
                .unwrap()
        })
        .await;
        assert_eq!(hours.len(), usize::try_from(DAYS * 24).unwrap());
        assert_eq!(hours[0].count, 720);
    }

    // A raw page from the middle of the month, and a whole day paged.
    let mid = Span {
        from: Some(at(START + 15 * DAY)),
        to: None,
    };
    let page = timed("a page of 1000 raw samples mid-month", async {
        store
            .samples(feeders[2], MetricKind::Power, mid, None, 1000)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(page.len(), 1000);
    let day = Span {
        from: Some(at(START + 20 * DAY)),
        to: Some(at(START + 21 * DAY)),
    };
    let (mut after, mut seen) = (None, 0);
    timed("a day of raw power samples in pages of 1000", async {
        loop {
            let page = store
                .samples(feeders[4], MetricKind::Power, day, after, 1000)
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.key);
            seen += page.len();
        }
    })
    .await;
    assert_eq!(seen, 17_280);

    // An hour stored since the roll-up is added in from the raw rows.
    let now = START + DAYS * DAY;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "WITH RECURSIVE t(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM t WHERE i < 719)
         INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, number, value)
         SELECT f.id, ?1 + i * 5000, 0, 'power', 4000, x'00' FROM t CROSS JOIN feeders AS f",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let hours = timed("a month of hours with the newest not rolled up", async {
        store
            .sample_hours(feeders[0], MetricKind::Power, Span::default(), None, 1000)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(hours.len(), usize::try_from(DAYS * 24 + 1).unwrap());
    assert_eq!(
        hours.last().map(|hour| (hour.count, hour.min)),
        Some((720, 4000))
    );
}
