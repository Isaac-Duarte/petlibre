//! The sample jobs: the rollups catch up on every hour stored since their
//! last run, and retention drops the raw samples past the owner's days
//! while their hours stay.

use std::num::NonZeroU32;

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    ClientRole, FeederId, HourSample, Job, JobOutcome, MetricKind, Resolution, SamplePage,
    SampleQuery, SampleValue,
};
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{TestService, config};
use wellfed_service::Service;
use wellfed_store::{NewFeeder, NewSample, SealedKey, Span, Store};
use uuid::Uuid;

/// `hours` before the fixed clock's time.
fn ago(hours: i64) -> Timestamp {
    FixedClock
        .now()
        .checked_sub(SignedDuration::from_hours(hours))
        .unwrap()
}

/// A feeder row with signal samples at each of `times`, written beside
/// the running service as ingest would. No feeder task runs for it; the
/// bytes are a stand-in, since only numbers are read here.
async fn feeder_with(store: &Store, times: &[(Timestamp, i32)]) -> FeederId {
    let id = FeederId(Uuid::from_u128(0x5a));
    store
        .insert_feeder(NewFeeder {
            id,
            identity: [5; 32],
            name: "kitchen",
            role: ClientRole::Server,
            address: None,
            key: SealedKey {
                nonce: [0; 12],
                sealed: vec![0],
            },
            created: ago(100),
        })
        .await
        .unwrap();
    let samples: Vec<_> = times
        .iter()
        .map(|&(received, dbm)| NewSample {
            received,
            uptime_ms: 0,
            value: SampleValue::Rssi { dbm },
            raw: vec![0],
        })
        .collect();
    store.record_samples(id, &samples).await.unwrap();
    id
}

async fn hours(service: &Service, id: FeederId) -> Vec<HourSample> {
    let query = SampleQuery {
        metric: MetricKind::Rssi,
        from: None,
        to: None,
        resolution: Some(Resolution::Hour),
        after: None,
        limit: None,
    };
    match service.samples(id, query).await.unwrap() {
        SamplePage::Hour { items, next } => {
            assert_eq!(next, None);
            items
        }
        page @ SamplePage::Raw { .. } => panic!("asked for hours: {page:?}"),
    }
}

async fn raw_count(store: &Store, id: FeederId) -> usize {
    store
        .samples(id, MetricKind::Rssi, Span::default(), None, 1000)
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn rollups_catch_up_and_retention_keeps_the_hours() {
    let mut config = config();
    config.raw_sample_days = NonZeroU32::new(1);
    let server = TestService::start_with(config).await;
    let service = server.service();
    let store = server.store().open().await;
    let id = feeder_with(
        &store,
        &[
            (ago(72), -40),
            (ago(72), -60),
            (ago(48), -50),
            (ago(1), -70),
        ],
    )
    .await;

    // Read from the raw rows before any roll-up: three hours.
    let before = hours(service, id).await;
    let counts: Vec<_> = before
        .iter()
        .map(|hour| (hour.count, hour.min, hour.max))
        .collect();
    assert_eq!(counts, [(2, -60, -40), (1, -50, -50), (1, -70, -70)]);
    assert!((before[0].mean + 50.0).abs() < f64::EPSILON);

    // The days the server missed are rolled up in one run.
    assert_eq!(
        service.run_job(Job::Rollups).await.outcome,
        JobOutcome::Succeeded
    );
    assert_eq!(hours(service, id).await, before);

    // A day's retention drops the two older days' raw rows, not the hours.
    assert_eq!(
        service.run_job(Job::Retention).await.outcome,
        JobOutcome::Succeeded
    );
    assert_eq!(raw_count(&store, id).await, 1);
    assert_eq!(hours(service, id).await, before);
    let jobs: Vec<_> = service
        .job_runs()
        .await
        .unwrap()
        .iter()
        .map(|run| run.job)
        .collect();
    assert_eq!(jobs, [Job::Retention, Job::Rollups]);

    store.close().await;
    server.shutdown().await;
}

#[tokio::test]
async fn without_a_retention_raw_samples_stay() {
    let server = TestService::start().await;
    let service = server.service();
    let store = server.store().open().await;
    let id = feeder_with(&store, &[(ago(24 * 400), -40), (ago(1), -50)]).await;

    service.run_retention(FixedClock.now()).await.unwrap();

    assert_eq!(raw_count(&store, id).await, 2);
    store.close().await;
    server.shutdown().await;
}
