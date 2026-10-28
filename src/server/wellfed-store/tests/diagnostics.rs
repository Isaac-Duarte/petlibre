//! Feed traces and statistics beside the history: a trace is kept by its
//! feed, only the recent feeds with none are asked for, the latest of each
//! statistics section and every food read are kept, and a projection
//! rebuild leaves all of it alone.

use jiff::Timestamp;
use tempfile::TempDir;
use uuid::Uuid;
use wellfed_api::{
    ClientRole, CurrentUnit, EventBody, FeedOutcome, FeedReason, FeederId, FeederRefusal,
    FoodLevel, FoodStats, Source, Stats, StatsSection, TraceSummary,
};
use wellfed_store::{NewEvent, NewFeeder, RawTrace, Refresh, SealedKey, Store, TraceFound};

const DATABASE: &str = "server.db";
const ID: FeederId = FeederId(Uuid::from_u128(1));

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn fresh() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join(DATABASE), at(0))
        .await
        .unwrap();
    store.refresh_projections(decode).await.unwrap();
    store
        .insert_feeder(NewFeeder {
            id: ID,
            identity: [1; 32],
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
    (dir, store)
}

fn decode(raw: &[u8]) -> Option<EventBody> {
    serde_json::from_slice(raw).ok()
}

/// Feed `feed`'s `Fed` event, as event `seq` of `epoch`.
async fn fed(store: &Store, epoch: u32, seq: u64, feed: u32) {
    let body = EventBody::Fed {
        feed,
        requested: 1,
        completed: 1,
        outcome: FeedOutcome::Done,
        reason: FeedReason::Done,
        hopper: FoodLevel::Present,
        by: Source::Panel,
    };
    let raw = serde_json::to_vec(&body).unwrap();
    store
        .record_event(
            ID,
            NewEvent {
                epoch,
                seq,
                at: None,
                received: at(1_000),
                body,
                raw: &raw,
            },
        )
        .await
        .unwrap();
}

fn trace(feed: u32, drive_ms: u32) -> TraceFound {
    TraceFound::Trace(RawTrace {
        summary: TraceSummary {
            feed,
            requested: 1,
            completed: 1,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
            drive_ms,
            samples: 0,
            unit: CurrentUnit::Scaled,
            current_peak: 0,
            current_mean: None,
            rising: 0,
            falling: 0,
            first_edge_ms: None,
            beam_breaks: 0,
            reversals: 0,
            battery_rest_mv: None,
            battery_min_mv: None,
            stride: 1,
            truncated: false,
        },
        stored: true,
        body: vec![1, 2, 3],
    })
}

#[tokio::test]
async fn a_trace_is_kept_by_its_feed_and_a_later_fetch_replaces_it() {
    let (_dir, store) = fresh().await;

    assert_eq!(store.trace(ID, None, 4).await.unwrap(), None);
    let missing = TraceFound::Missing(FeederRefusal::NotFound);
    store
        .record_trace(ID, 1, 4, at(10), &missing)
        .await
        .unwrap();
    store
        .record_trace(ID, 2, 4, at(20), &trace(4, 200))
        .await
        .unwrap();

    let newest = store.trace(ID, None, 4).await.unwrap().unwrap();
    assert_eq!((newest.epoch, newest.fetched), (2, at(20)));
    assert_eq!(newest.found, trace(4, 200));
    let older = store.trace(ID, Some(1), 4).await.unwrap().unwrap();
    assert_eq!(older.found, missing);

    store
        .record_trace(ID, 1, 4, at(30), &trace(4, 300))
        .await
        .unwrap();
    let replaced = store.trace(ID, Some(1), 4).await.unwrap().unwrap();
    assert_eq!(replaced.found, trace(4, 300));
    assert_eq!(store.trace(ID, Some(3), 4).await.unwrap(), None);
}

#[tokio::test]
async fn only_the_recent_feeds_with_no_trace_are_asked_for() {
    let (_dir, store) = fresh().await;
    assert!(store.untraced_feeds(ID, 1, 16).await.unwrap().is_empty());

    for feed in 1..=20 {
        fed(&store, 1, u64::from(feed), feed).await;
    }
    fed(&store, 2, 1, 21).await;
    store
        .record_trace(ID, 1, 20, at(5), &trace(20, 1))
        .await
        .unwrap();
    store
        .record_trace(
            ID,
            1,
            18,
            at(5),
            &TraceFound::Missing(FeederRefusal::NotFound),
        )
        .await
        .unwrap();

    let want: Vec<u32> = [19, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5].into();
    assert_eq!(store.untraced_feeds(ID, 1, 16).await.unwrap(), want);
    assert_eq!(store.untraced_feeds(ID, 2, 16).await.unwrap(), [21]);
}

#[tokio::test]
async fn the_latest_of_each_section_and_every_food_read_are_kept() {
    let (_dir, store) = fresh().await;
    let food = |present| FoodStats {
        present: Some(present),
        emptied: 1,
        refilled: u32::from(present),
        feeds_since_refill: Some(3),
    };

    store
        .record_stats(ID, at(100), Some(4), &Stats::Food(food(false)))
        .await
        .unwrap();
    store
        .record_stats(ID, at(100), Some(4), &Stats::Counters(Default::default()))
        .await
        .unwrap();
    store
        .record_stats(ID, at(200), Some(5), &Stats::Food(food(true)))
        .await
        .unwrap();

    let latest = store.latest_stats(ID).await.unwrap();
    let sections: Vec<_> = latest.iter().map(|row| row.stats.section()).collect();
    assert_eq!(sections, [StatsSection::Counters, StatsSection::Food]);
    assert_eq!(latest[1].stats, Stats::Food(food(true)));
    assert_eq!(
        (latest[1].received, latest[1].boot_count),
        (at(200), Some(5))
    );

    let readings = store.food_readings(ID, at(0), at(1_000)).await.unwrap();
    let foods: Vec<_> = readings.iter().map(|read| read.food).collect();
    assert_eq!(foods, [food(false), food(true)]);
    assert_eq!(readings[0].boot_count, Some(4));
    let later = store.food_readings(ID, at(150), at(1_000)).await.unwrap();
    assert_eq!(later.len(), 1, "a window of receive times");
}

#[tokio::test]
async fn a_projection_rebuild_leaves_the_traces_and_statistics() {
    let (dir, store) = fresh().await;
    fed(&store, 1, 1, 1).await;
    store
        .record_trace(ID, 1, 1, at(5), &trace(1, 9))
        .await
        .unwrap();
    store
        .record_stats(ID, at(5), None, &Stats::Food(FoodStats::default()))
        .await
        .unwrap();

    let url = format!("sqlite://{}", dir.path().join(DATABASE).display());
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE projection_version SET version = 0")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    assert!(matches!(
        store.refresh_projections(decode).await.unwrap(),
        Refresh::Rebuilt { events: 1, .. }
    ));

    let kept = store.trace(ID, None, 1).await.unwrap().unwrap();
    assert_eq!(kept.found, trace(1, 9));
    assert_eq!(store.latest_stats(ID).await.unwrap().len(), 1);
    let readings = store.food_readings(ID, at(0), at(10)).await.unwrap();
    assert_eq!(readings.len(), 1);
}
