//! The server end to end: real HTTP in front, and behind it the service
//! speaking the device API (Noise over TCP) to the simulated feeder.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use jiff::tz::TimeZone;
use wellfed_api::{
    Connection, EventBody, EventPage, Feeder, FeederDetail, Settings, Source, StreamEvent,
};
use wellfed_client::api::Invite;
use wellfed_core::noise::Keypair;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::Running;
use wellfed_proto::api::{Op, Outcome, Role};
use wellfed_service::{Config, Discovery, Service, SystemClock, SystemRandom, Timing};
use wellfed_store::Store;
use reqwest::StatusCode;
use reqwest::header::{ETAG, HeaderMap, HeaderValue, IF_MATCH, ORIGIN};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;

const OWNER: [u8; 32] = [0x21; 32];
const WAIT: Duration = Duration::from_secs(10);

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

/// The server on a free localhost port.
async fn serve(dir: &tempfile::TempDir) -> (SocketAddr, Service, CancellationToken) {
    let store = Store::open(&dir.path().join("server.db")).await.unwrap();
    let config = Config {
        key: [7; 32],
        zone: TimeZone::UTC,
        label: "test-server".into(),
        discovery: Discovery::Off,
        timing: Timing {
            retry_min: Duration::from_millis(50),
            retry_max: Duration::from_millis(400),
            ..Timing::default()
        },
    };
    let service = Service::start(store, config, Arc::new(SystemClock), Arc::new(SystemRandom))
        .await
        .unwrap();
    let shutdown = CancellationToken::new();
    let app = wellfed_server::app(service.clone(), shutdown.clone(), Default::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await
            .unwrap();
    });
    (address, service, shutdown)
}

/// A browser on the server's own page: its cookies and `Origin`.
fn browser(server: SocketAddr) -> reqwest::Client {
    let mut headers = HeaderMap::new();
    let origin = HeaderValue::from_str(&format!("http://{server}")).unwrap();
    headers.insert(ORIGIN, origin);
    reqwest::Client::builder()
        .cookie_store(true)
        .default_headers(headers)
        .build()
        .unwrap()
}

/// An invite for the server, as the owner's CLI would make it.
async fn server_invite(sim: &Running) -> String {
    let owner = Keypair::from_secret(OWNER);
    let mut connection =
        wellfed_connect::Connection::open(&owner, sim.identity, sim.address, [0x99; 32])
            .await
            .unwrap();
    let answer = connection
        .ask([0x44; 16], Op::CreateInvite { role: Role::Server })
        .await
        .unwrap();
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

/// The stream's updates, parsed, as they arrive, each with its SSE `id`.
fn follow(response: reqwest::Response) -> mpsc::UnboundedReceiver<(Option<String>, StreamEvent)> {
    let (sink, updates) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut body = response.bytes_stream();
        let mut text = String::new();
        while let Some(Ok(chunk)) = body.next().await {
            text.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = text.find("\n\n") {
                let block: String = text.drain(..end + 2).collect();
                // A block's fields come in any order.
                let (mut id, mut data) = (None, None);
                for line in block.lines() {
                    if let Some(value) = line.strip_prefix("id:") {
                        id = Some(value.trim().to_owned());
                    }
                    if let Some(value) = line.strip_prefix("data:") {
                        data = Some(value.trim().to_owned());
                    }
                }
                let Some(data) = data else {
                    continue;
                };
                let update = serde_json::from_str(&data).unwrap();
                if sink.send((id, update)).is_err() {
                    return;
                }
            }
        }
    });
    updates
}

#[test]
fn an_owner_adopts_a_feeder_and_works_it_over_http() {
    local(async {
        let sim = Running::start(
            &Flash::memory(),
            Some(*Keypair::from_secret(OWNER).public()),
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        let (server, service, shutdown) = serve(&dir).await;
        let http = browser(server);
        let url = |path: &str| format!("http://{server}/api/v1{path}");

        let setup = http
            .post(url("/setup"))
            .json(&json!({ "name": "ada", "password": "correct horse" }))
            .send()
            .await
            .unwrap();
        assert_eq!(setup.status(), StatusCode::CREATED);

        let adopted = http
            .post(url("/feeders"))
            .json(&json!({
                "invite": server_invite(&sim).await,
                "name": "kitchen",
                "address": sim.address.to_string(),
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(adopted.status(), StatusCode::CREATED);
        let feeder: Feeder = adopted.json().await.unwrap();
        let base = url(&format!("/feeders/{}", feeder.id));

        tokio::time::timeout(WAIT, async {
            loop {
                let feeders: Vec<Feeder> = http
                    .get(url("/feeders"))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                if feeders[0].connection == Connection::Online {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the server connects to the feeder");

        let stream = http.get(url("/stream?samples=power")).send().await.unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        let mut updates = follow(stream);

        let detail: FeederDetail = http.get(&base).send().await.unwrap().json().await.unwrap();
        let status = detail.status.unwrap();
        assert!(status.live);
        assert!(status.status.clock.is_some(), "the server set the clock");

        // Settings: an edit names its generation; a retry with the same
        // key is answered once; a stale edit gets the current settings.
        let read = http.get(format!("{base}/settings")).send().await.unwrap();
        let etag = read.headers()[ETAG].clone();
        let settings: Settings = read.json().await.unwrap();
        assert_eq!(
            etag.to_str().unwrap(),
            format!("\"{}\"", settings.generation)
        );
        let key = "0192c5d0-1111-7000-8000-000000000001";
        let patch = |if_match: &HeaderValue, key: &str| {
            http.patch(format!("{base}/settings"))
                .header(IF_MATCH, if_match)
                .header("Idempotency-Key", key)
                .json(&json!({ "volume": 35 }))
                .send()
        };
        let changed = patch(&etag, key).await.unwrap();
        assert_eq!(changed.status(), StatusCode::OK);
        let changed: Settings = changed.json().await.unwrap();
        assert_eq!(changed.volume, 35);
        let retried: Settings = patch(&etag, key).await.unwrap().json().await.unwrap();
        assert_eq!(retried, changed);
        let stale = patch(&etag, "0192c5d0-1111-7000-8000-000000000002")
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(stale.json::<Settings>().await.unwrap(), changed);

        let lock = http
            .post(format!("{base}/lock"))
            .header("Idempotency-Key", "0192c5d0-1111-7000-8000-000000000003")
            .json(&json!({ "locked": false }))
            .send()
            .await
            .unwrap();
        assert_eq!(lock.status(), StatusCode::NO_CONTENT);

        // The lock's event and a power sample reach the stream.
        let (mut lock_seen, mut power_seen, mut first) = (None, false, None);
        tokio::time::timeout(WAIT, async {
            while !(lock_seen.is_some() && power_seen) {
                let (id, update) = updates.recv().await.unwrap();
                if first.is_none() {
                    first.clone_from(&id);
                }
                match update {
                    StreamEvent::Event { event, .. }
                        if matches!(event.body, EventBody::Lock { locked: false, .. }) =>
                    {
                        lock_seen = id;
                    }
                    StreamEvent::Sample { .. } => power_seen = true,
                    _ => {}
                }
            }
        })
        .await
        .expect("the lock and a sample arrive live");

        // A reconnect from the first update gets everything after it, the
        // lock among them; one from another run is told to resync.
        let first = first.unwrap();
        let resumed = http
            .get(url("/stream"))
            .header("Last-Event-ID", &first)
            .send()
            .await
            .unwrap();
        let mut replay = follow(resumed);
        tokio::time::timeout(WAIT, async {
            loop {
                let (id, update) = replay.recv().await.unwrap();
                assert_ne!(id.as_ref(), Some(&first), "replay starts after the id");
                assert!(
                    !matches!(update, StreamEvent::Sample { .. }),
                    "not asked for"
                );
                if id == lock_seen {
                    return;
                }
            }
        })
        .await
        .expect("the replay includes the lock");
        let stale = http
            .get(url("/stream"))
            .header("Last-Event-ID", "0000000000000001-0")
            .send()
            .await
            .unwrap();
        let (id, update) = follow(stale).recv().await.unwrap();
        assert_eq!((id.as_deref(), update), (Some(""), StreamEvent::Resync));

        let page: EventPage = http
            .get(format!("{base}/events?kind=lock"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let [event] = page.events.as_slice() else {
            panic!("{page:?}");
        };
        assert!(matches!(
            event.body,
            EventBody::Lock {
                locked: false,
                by: Source::Client { .. }
            }
        ));

        shutdown.cancel();
        service.shutdown().await;
        sim.stop().await;
    });
}
