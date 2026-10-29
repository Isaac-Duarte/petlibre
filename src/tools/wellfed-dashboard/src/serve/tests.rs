use super::*;
use axum::body::Body;
use futures_util::StreamExt;
use tower::ServiceExt;

use crate::Live;
use crate::views::tests::idle;
use wellfed_formats::bench::CAPTURES;
use wellfed_formats::bench::live::{CancelState, JobView, Uart};

/// The address the test page answers as.
const HOST: &str = "127.0.0.1:8780";

/// A server's state and one job's output, set by the test.
struct Fake {
    state: watch::Sender<BenchState>,
    output: Arc<Lines>,
}

struct Lines {
    lines: Mutex<Vec<String>>,
    changed: watch::Sender<u64>,
}

impl Lines {
    fn push(&self, line: &str) {
        self.lines.lock().unwrap().push(line.into());
        self.changed.send_modify(|n| *n += 1);
    }
}

impl JobOutput for Lines {
    fn read_from(&self, cursor: usize, most: usize) -> (Vec<String>, usize) {
        let lines = self.lines.lock().unwrap();
        let from = cursor.min(lines.len());
        let to = (from + most).min(lines.len());
        (lines[from..to].to_vec(), to)
    }

    fn end(&self) -> usize {
        self.lines.lock().unwrap().len()
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
}

impl Live for Fake {
    fn subscribe(&self) -> watch::Receiver<BenchState> {
        self.state.subscribe()
    }

    fn output(&self, id: JobId) -> Option<Arc<dyn JobOutput>> {
        (id == 7).then(|| Arc::clone(&self.output) as Arc<dyn JobOutput>)
    }
}

fn ui() -> (tempfile::TempDir, Arc<Fake>, Router) {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake {
        state: watch::Sender::new(idle()),
        output: Arc::new(Lines {
            lines: Mutex::new(Vec::new()),
            changed: watch::Sender::new(0),
        }),
    });
    let ui = Ui {
        sources: Sources::new(
            tmp.path().into(),
            tmp.path().join("wellfed-flash"),
            Arc::clone(&fake) as Arc<dyn Live>,
        ),
        flash: Arc::default(),
        hosts: Hosts {
            ip: "127.0.0.1".parse().unwrap(),
            port: 8780,
        },
        stop: Cancel::new(),
    };
    (tmp, fake, router(ui))
}

async fn get_as(router: &Router, uri: &str, host: &str) -> Response {
    let request = Request::get(uri)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(request).await.unwrap()
}

async fn get(router: &Router, uri: &str) -> Response {
    get_as(router, uri, HOST).await
}

/// Every view that reads a file or asks the stock server.
const FILE_VIEWS: [&str; 6] = [
    "/api/evidence",
    "/api/sessions",
    "/api/health",
    "/api/tests",
    "/api/stock",
    "/api/tests/absent",
];

#[tokio::test]
async fn loopback_serves_the_page_and_api() {
    let (_tmp, _fake, router) = ui();
    let index = get(&router, "/").await;
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(
        index.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );

    // Every embedded file is served under its own path with a known type:
    // the build's hashed scripts, styles and fonts.
    let mut kinds = Vec::new();
    for path in Assets::iter() {
        let response = get(&router, &format!("/{path}")).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let kind = response.headers()[header::CONTENT_TYPE].to_str().unwrap();
        assert_ne!(kind, "application/octet-stream", "{path}");
        kinds.push(kind.to_string());
    }
    for kind in ["text/javascript", "text/css", "font/woff2"] {
        assert!(kinds.iter().any(|k| k.starts_with(kind)), "no {kind}");
    }

    for uri in [
        "/api/state",
        "/api/evidence",
        "/api/sessions",
        "/api/health",
        "/api/tests",
        "/api/stock",
    ] {
        assert_eq!(get(&router, uri).await.status(), StatusCode::OK, "{uri}");
    }

    assert_eq!(
        get(&router, "/missing").await.status(),
        StatusCode::NOT_FOUND
    );
    for uri in ["/api/sessions/absent", "/api/tests/absent"] {
        assert_eq!(get(&router, uri).await.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn while_the_flash_client_holds_the_uart_every_file_view_is_deferred() {
    let (tmp, fake, router) = ui();
    // Evidence a view would otherwise read.
    let capture = tmp.path().join(CAPTURES).join("c");
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::write(capture.join("serial-rx.bin"), b"hello\r\n").unwrap();

    for uart in [
        Uart::Writing {
            done: 3,
            total: 19,
            counted: true,
        },
        Uart::Loader,
    ] {
        fake.state.send_modify(|state| state.uart = uart.clone());

        for uri in FILE_VIEWS {
            let response = get(&router, uri).await;
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri} {uart:?}"
            );
            assert_eq!(response.headers()[header::RETRY_AFTER], DEFERRED_RETRY);
        }
        // The state is memory, and always answers.
        assert_eq!(get(&router, "/api/state").await.status(), StatusCode::OK);
    }

    fake.state.send_modify(|state| state.uart = Uart::Idle);
    assert_eq!(get(&router, "/api/evidence").await.status(), StatusCode::OK);
}

/// Read the stream until `want` is in what arrived, or fail after 5 s.
async fn read_until(
    body: &mut (impl Stream<Item = std::result::Result<axum::body::Bytes, axum::Error>> + Unpin),
    seen: &mut String,
    want: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !seen.contains(want) {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .unwrap_or_else(|_| panic!("no {want:?} in {seen}"))
            .unwrap()
            .unwrap();
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
}

/// Whatever the stream sends within `wait`.
async fn read_for(
    body: &mut (impl Stream<Item = std::result::Result<axum::body::Bytes, axum::Error>> + Unpin),
    seen: &mut String,
    wait: Duration,
) {
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, body.next()).await {
        seen.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
    }
}

#[tokio::test]
async fn the_stream_sends_the_state_then_the_jobs_output_as_it_comes() {
    let (_tmp, fake, router) = ui();
    fake.output.push("before the page connected");
    let response = get(&router, "/api/events").await;
    let mut body = response.into_body().into_data_stream();
    let mut seen = String::new();

    read_until(&mut body, &mut seen, "event: evidence").await;
    assert!(seen.starts_with("event: state\n"), "{seen}");
    // With data, or a browser would never dispatch it.
    assert!(seen.contains("event: log-reset\ndata: none\n"), "{seen}");

    fake.state.send_modify(|state| {
        state.job = Some(JobView {
            id: 7,
            label: "capture c1".into(),
            log: "private/bench-status/jobs/7-c1.log".into(),
            accepted: 1.0,
            record: None,
            upcoming: Vec::new(),
            cancel: CancelState::None,
            end: None,
        });
    });
    read_until(&mut body, &mut seen, "before the page connected").await;
    assert!(seen.contains(r#""label":"capture c1""#), "{seen}");

    fake.output.push("a new line");
    read_until(&mut body, &mut seen, "data: a new line").await;
}

#[tokio::test]
async fn a_page_that_connects_during_a_write_gets_the_state_and_output_but_reads_no_file() {
    let (_tmp, fake, router) = ui();
    fake.state.send_modify(|state| {
        state.uart = Uart::Writing {
            done: 3,
            total: 19,
            counted: true,
        };
        state.job = Some(JobView {
            id: 7,
            label: "flash-execute u1".into(),
            log: "private/bench-status/jobs/7-u1.log".into(),
            accepted: 1.0,
            record: None,
            upcoming: Vec::new(),
            cancel: CancelState::None,
            end: None,
        });
    });
    fake.output.push("Verified changed sectors: 3/19");
    let response = get(&router, "/api/events").await;
    let mut body = response.into_body().into_data_stream();
    let mut seen = String::new();

    read_until(&mut body, &mut seen, "Verified changed sectors").await;
    read_for(&mut body, &mut seen, 4 * TICK).await;
    assert!(seen.starts_with("event: state\n"), "{seen}");
    assert!(!seen.contains("event: evidence"), "{seen}");
    assert!(!seen.contains("event: stock"), "{seen}");

    // The write ends: the files are read again.
    fake.state.send_modify(|state| state.uart = Uart::Idle);
    read_until(&mut body, &mut seen, "event: evidence").await;
}

#[tokio::test]
async fn a_page_far_behind_catches_up_in_batches() {
    let (_tmp, fake, router) = ui();
    for n in 0..(LOG_BATCH + 10) {
        fake.output.push(&format!("line {n}"));
    }
    fake.state.send_modify(|state| {
        state.job = Some(JobView {
            id: 7,
            label: "dump d1".into(),
            log: "private/bench-status/jobs/7-d1.log".into(),
            accepted: 1.0,
            record: None,
            upcoming: Vec::new(),
            cancel: CancelState::None,
            end: None,
        });
    });
    let response = get(&router, "/api/events").await;
    let mut body = response.into_body().into_data_stream();
    let mut seen = String::new();

    read_until(&mut body, &mut seen, &format!("line {}\n", LOG_BATCH + 9)).await;

    assert!(seen.contains("data: line 0\n"), "the first batch");
    assert_eq!(seen.matches("event: log\n").count(), 2);
}

#[test]
fn only_this_pages_own_address_or_localhost_is_answered() {
    let loopback = Hosts {
        ip: "127.0.0.1".parse().unwrap(),
        port: 8780,
    };
    for host in [
        "127.0.0.1:8780",
        "localhost:8780",
        "LOCALHOST:8780",
        "[::1]:8780",
    ] {
        assert!(loopback.allows(host), "{host}");
    }
    for host in [
        "evil.example:8780",
        "127.0.0.1:8781",
        "127.0.0.1",
        "100.64.0.1:8780",
        "localhost:",
        "[::1",
        "",
    ] {
        assert!(!loopback.allows(host), "{host}");
    }

    let tailnet = Hosts {
        ip: "100.64.0.1".parse().unwrap(),
        port: 8780,
    };
    assert!(tailnet.allows("100.64.0.1:8780"));
    assert!(!tailnet.allows("100.64.0.2:8780"));

    // A wildcard bind is reached by any of the host's addresses, never by
    // a name another site controls.
    let every = Hosts {
        ip: "0.0.0.0".parse().unwrap(),
        port: 8780,
    };
    assert!(every.allows("192.168.4.20:8780"));
    assert!(!every.allows("rebound.example:8780"));
}

#[tokio::test]
async fn a_request_addressed_to_another_name_is_refused() {
    let (_tmp, _fake, router) = ui();

    for uri in ["/", "/api/state", "/api/events"] {
        let response = get_as(&router, uri, "rebound.example:8780").await;
        assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST, "{uri}");
    }
    let unaddressed = router
        .clone()
        .oneshot(Request::get("/api/state").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unaddressed.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[test]
fn listen_takes_literal_addresses() {
    assert_eq!(
        Listen::new("0.0.0.0", 8780).unwrap().url(),
        "http://0.0.0.0:8780/"
    );
    assert!(Listen::new("localhost", 8780).is_err());
    assert_eq!(
        Listen::new("100.64.0.1", 8780).unwrap().url(),
        "http://100.64.0.1:8780/"
    );

    assert!(
        Listen::new("::1", 8780)
            .unwrap()
            .url()
            .starts_with("http://[::1]:8780/")
    );
}
