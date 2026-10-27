//! A feeder's samples over HTTP: raw with their numbers, in pages, and by
//! the hour. The jobs that keep them are the service's tests.

use std::time::Duration;

use jiff::SignedDuration;
use wellfed_api::{
    AdoptFeeder, ClientRole, Connection, ErrorCode, FeederId, MetricKind, PREFIX, RawSample,
    Resolution, SamplePage, SampleQuery, sample_number,
};
use wellfed_server_client::{Client, Method, StatusCode};
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{
    TestServer, WAIT, ada_setup, feeder, local, refused, refused_for, until, until_within,
};

/// How often the simulator sends a power sample.
const POWER_PERIOD: Duration = Duration::from_secs(5);

fn query(resolution: Resolution) -> SampleQuery {
    SampleQuery {
        metric: MetricKind::Power,
        from: None,
        to: None,
        resolution: Some(resolution),
        after: None,
        limit: None,
    }
}

async fn raw(http: &Client, id: FeederId, query: &SampleQuery) -> (Vec<RawSample>, Option<String>) {
    match http.samples(id, query).await.unwrap() {
        SamplePage::Raw { items, next } => (items, next),
        page @ SamplePage::Hour { .. } => panic!("asked for raw: {page:?}"),
    }
}

/// An owner, the simulator adopted, and its first `n` power samples
/// stored.
async fn mirrored(server: &TestServer, sim: &feeder::Running, n: usize) -> (Client, FeederId) {
    let mut http = server.client();
    http.setup(&ada_setup()).await.unwrap();
    let id = http
        .adopt(&AdoptFeeder {
            invite: feeder::invite(sim, ClientRole::Server).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        })
        .await
        .unwrap()
        .id;
    until("the server connects to the feeder", || async {
        http.feeders().await.unwrap()[0].connection == Connection::Online
    })
    .await;
    // Each sample is written within the test config's batch wait, but the
    // simulator only sends one every period: `n` of them take `n` periods
    // before the server can store the last.
    let mut all = query(Resolution::Raw);
    all.limit = Some(1000);
    let periods = u32::try_from(n).expect("a test asks for a few samples");
    let limit = WAIT + POWER_PERIOD * periods;
    until_within("power samples are stored", limit, || async {
        raw(&http, id, &all).await.0.len() >= n
    })
    .await;
    (http, id)
}

#[test]
fn samples_read_raw_in_pages_and_by_the_hour() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let (http, id) = mirrored(&server, &sim, 2).await;

        let mut first = query(Resolution::Raw);
        first.limit = Some(1);
        let (items, next) = raw(&http, id, &first).await;
        let [sample] = items.as_slice() else {
            panic!("{items:?}")
        };
        assert_eq!(sample.value.metric(), MetricKind::Power);
        assert_eq!(sample.number, sample_number(&sample.value));
        assert_eq!(sample.received, FixedClock.now());
        let next = next.expect("another page follows");
        let (rest, _) = raw(
            &http,
            id,
            &SampleQuery {
                after: Some(next),
                limit: Some(1000),
                ..first.clone()
            },
        )
        .await;
        assert!(!rest.is_empty());
        assert!(!rest.contains(sample), "a page starts after the last");

        // The simulator runs on the adapter with no battery: its power
        // samples have no number, so no hour holds them.
        assert_eq!(sample.number, None);
        let page = http.samples(id, &query(Resolution::Hour)).await.unwrap();
        assert_eq!(
            page,
            SamplePage::Hour {
                items: Vec::new(),
                next: None
            }
        );

        // Another metric, or a span that leaves them out, has none.
        let mut heap = query(Resolution::Raw);
        heap.metric = MetricKind::Heap;
        assert!(raw(&http, id, &heap).await.0.is_empty());
        let mut later = query(Resolution::Raw);
        later.from = Some(
            FixedClock
                .now()
                .checked_add(SignedDuration::from_secs(1))
                .unwrap(),
        );
        assert!(raw(&http, id, &later).await.0.is_empty());

        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn a_bad_sample_query_is_refused() {
    local(async {
        let server = TestServer::start().await;
        let mut http = server.client();
        http.setup(&ada_setup()).await.unwrap();

        let unknown = FeederId(uuid::Uuid::from_u128(9));
        refused(
            http.samples(unknown, &query(Resolution::Raw)).await,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        );
        let path = format!("{PREFIX}/feeders/{}/samples", unknown.0);
        for query in [
            "",
            "metric=weight",
            "metric=power&limit=0",
            "metric=power&limit=1001",
            "metric=power&after=7",
            "metric=power&resolution=hour&after=x",
            "metric=power&resolution=minute",
            "metric=power&from=2026-10-27T10:00:00Z&to=2026-10-27T10:00:00Z",
        ] {
            let response = http
                .request(Method::GET, &format!("{path}?{query}"))
                .send()
                .await
                .unwrap();
            let result = wellfed_server_client::expect_status(response, StatusCode::OK).await;
            refused_for(query, result, StatusCode::BAD_REQUEST, ErrorCode::Invalid);
        }
        server.stop().await;
    });
}
