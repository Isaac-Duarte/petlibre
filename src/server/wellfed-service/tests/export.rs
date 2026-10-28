//! An export of a large table streams: the archive comes in chunks of at
//! most [`EXPORT_CHUNK`] bytes as they are read, from one snapshot, while
//! the database takes writes; one runs at a time; and an import whose
//! upload goes quiet ends.

use std::pin::pin;
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;
use wellfed_api::{ClientRole, ExportFormat, FeederId, SampleValue};
use wellfed_formats::export::{Manifest, Table};
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{TestService, config};
use wellfed_service::{EXPORT_CHUNK, Error, ExportError};
use wellfed_store::{NewFeeder, NewSample, SealedKey, Store};

/// Far more samples than the queue holds: about 13 MB of archive.
const SAMPLES: usize = 100_000;

async fn feeder(store: &Store) -> FeederId {
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
            created: FixedClock.now(),
        })
        .await
        .unwrap();
    id
}

fn sample(n: usize) -> NewSample {
    let seconds = i64::try_from(n).unwrap();
    NewSample {
        received: Timestamp::UNIX_EPOCH + SignedDuration::from_secs(1_790_000_000 + seconds),
        uptime_ms: 0,
        value: SampleValue::Rssi { dbm: -60 },
        raw: vec![0x77],
    }
}

#[tokio::test]
async fn a_large_table_streams_in_bounded_chunks_from_one_snapshot() {
    let server = TestService::start().await;
    let store = server.store().open().await;
    let id = feeder(&store).await;
    let samples: Vec<_> = (0..SAMPLES).map(sample).collect();
    for batch in samples.chunks(10_000) {
        store.record_samples(id, batch).await.unwrap();
    }

    let chunks = server
        .service()
        .export(ExportFormat::Json, "test")
        .await
        .unwrap();
    let mut chunks = pin!(chunks);
    let first = chunks.try_next().await.unwrap().unwrap();

    // The snapshot is taken: a write now goes through, and is not in it.
    store.record_samples(id, &[sample(SAMPLES)]).await.unwrap();

    let mut archive = first.clone();
    let mut largest = first.len();
    let mut count = 1;
    while let Some(chunk) = chunks.try_next().await.unwrap() {
        largest = largest.max(chunk.len());
        count += 1;
        archive.extend_from_slice(&chunk);
    }
    assert!(largest <= EXPORT_CHUNK, "a chunk of {largest} bytes");
    assert!(count > 100, "{count} chunks for {} bytes", archive.len());
    assert!(archive.len() > 100 * EXPORT_CHUNK);

    let size = std::str::from_utf8(&archive[124..135]).unwrap();
    let size = usize::from_str_radix(size, 8).unwrap();
    let manifest: Manifest = serde_json::from_slice(&archive[512..512 + size]).unwrap();
    let samples = manifest
        .tables
        .iter()
        .find(|file| file.table == Table::Samples)
        .unwrap();
    assert_eq!(samples.rows, u64::try_from(SAMPLES).unwrap());

    store.close().await;
    server.shutdown().await;
}

#[tokio::test]
async fn an_export_left_unread_ends_with_the_service() {
    let server = TestService::start().await;
    let store = server.store().open().await;
    let id = feeder(&store).await;
    let samples: Vec<_> = (0..20_000).map(sample).collect();
    store.record_samples(id, &samples).await.unwrap();

    // The caller reads one chunk and then nothing: the writer waits on the
    // full queue, and stopping the service ends it rather than waiting.
    let chunks = server
        .service()
        .export(ExportFormat::Csv, "test")
        .await
        .unwrap();
    let mut chunks = pin!(chunks);
    chunks.try_next().await.unwrap().unwrap();

    // It holds the one export slot meanwhile.
    let second = server.service().export(ExportFormat::Json, "test").await;
    assert!(
        matches!(second, Err(Error::Export(ExportError::Running))),
        "a second export while one runs"
    );

    store.close().await;
    tokio::time::timeout(std::time::Duration::from_secs(10), server.shutdown())
        .await
        .expect("the shutdown does not wait for the reader");
}

#[tokio::test]
async fn an_upload_gone_quiet_ends_the_import_and_frees_the_writer() {
    let mut config = config();
    config.timing.upload_idle = Duration::from_millis(200);
    let server = TestService::start_with(config).await;
    let service = server.service();

    // Half a block, then nothing.
    let quiet = futures_util::stream::iter([Ok::<_, std::io::Error>(vec![0; 256])])
        .chain(futures_util::stream::pending());
    let refused = service.import(Box::pin(quiet)).await;

    assert!(
        matches!(refused, Err(Error::Export(ExportError::Upload(_)))),
        "{refused:?}"
    );
    assert!(service.setup_open().await.unwrap(), "nothing was kept");

    server.shutdown().await;
}
