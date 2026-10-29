//! The page's HTTP server: the built web app from `src/web/apps/bench/dist`,
//! embedded at compile time, the JSON views it reads, and one event stream
//! of the server's live state, the running job's output and the views that
//! change with it.
//!
//! The page shows private UART output, so it binds loopback by default.
//! `--listen` serves another address: a named interface such as the
//! Tailscale address, or a wildcard for every interface when the owner
//! chooses to.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use futures_util::stream::{self, Stream};
use rust_embed::Embed;
use serde::Serialize;
use tokio::sync::watch;

use super::feeder::{self, FeederView};
use super::runs;
use super::views::evidence::{self, EvidenceView};
use super::views::health::{self, FlashCache};
use super::views::{Sources, sessions};

use wellfed_formats::bench::live::{BenchState, JobId, Uart};
use wellfed_formats::bench::{Cancel, now};

use crate::{Error, JobOutput, Result};

#[derive(Embed)]
// Relative to this crate; `make ui` builds it.
#[folder = "../../../src/web/apps/bench/dist/"]
struct Assets;

/// A newly connected page gets at most this many of the job's lines, the
/// most the page keeps.
const LOG_BACKLOG: usize = 20_000;
/// How often the evidence files are read again for a connected page.
const EVIDENCE_EVERY: Duration = Duration::from_secs(1);
/// How often the event stream asks the local stock server about the feeder.
const FEEDER_EVERY: Duration = Duration::from_secs(5);
/// How often a quiet stream looks at the clocks above and the stop flag.
const TICK: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct Ui {
    sources: Sources,
    flash: Arc<Mutex<FlashCache>>,
    /// Cancelled when the process is stopping: event streams end, then the
    /// server.
    stop: Cancel,
}

impl Ui {
    fn stopping(&self) -> bool {
        self.stop.is_cancelled()
    }
}

/// Where the page listens.
pub struct Listen {
    address: SocketAddr,
}

impl Listen {
    pub fn new(listen: &str, port: u16) -> Result<Self> {
        let ip: IpAddr = listen.parse().map_err(|_| Error::Listen)?;
        Ok(Self {
            address: SocketAddr::new(ip, port),
        })
    }

    pub fn url(&self) -> String {
        format!("http://{}/", self.address)
    }
}

fn router(ui: Ui) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(state))
        .route("/api/events", get(events))
        .route("/api/evidence", get(evidence_view))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/{name}", get(session))
        .route("/api/health", get(health))
        .route("/api/tests", get(tests))
        .route("/api/tests/{name}", get(test_run))
        .route("/api/feeder", get(feeder_status))
        .fallback(asset)
        .layer(middleware::from_fn(no_store))
        .with_state(ui)
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Run a blocking view on the blocking pool; `None` is a missing page.
async fn view<T: Serialize + Send + 'static>(
    ui: &Ui,
    make: impl FnOnce(&Ui) -> Option<T> + Send + 'static,
) -> Response {
    let ui = ui.clone();
    match tokio::task::spawn_blocking(move || make(&ui)).await {
        Ok(Some(view)) => Json(view).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The server's state as of now.
fn current(ui: &Ui) -> BenchState {
    let mut state = ui.sources.live.subscribe().borrow().clone();
    state.now = now();
    state
}

async fn state(State(ui): State<Ui>) -> Json<BenchState> {
    Json(current(&ui))
}

async fn evidence_view(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(evidence::evidence(&ui.sources))).await
}

async fn sessions(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(sessions::sessions(&ui.sources))).await
}

async fn session(State(ui): State<Ui>, UrlPath(name): UrlPath<String>) -> Response {
    view(&ui, move |ui| sessions::session(&ui.sources, &name)).await
}

async fn health(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| {
        // The cache holds only a hash; one left mid-update by a panicked
        // view is recomputed when the binary next changes, so take it as is.
        let mut flash = ui.flash.lock().unwrap_or_else(PoisonError::into_inner);
        Some(health::health(&ui.sources, &mut flash))
    })
    .await
}

async fn tests(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(runs::tests(&ui.sources.root))).await
}

async fn test_run(State(ui): State<Ui>, UrlPath(name): UrlPath<String>) -> Response {
    view(&ui, move |ui| runs::run(&ui.sources.root, &name)).await
}

async fn feeder_status(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(feeder::feeder(&ui.sources.root))).await
}

async fn index() -> Response {
    serve_asset("index.html")
}

async fn asset(uri: Uri) -> Response {
    serve_asset(uri.path().trim_start_matches('/'))
}

fn serve_asset(path: &str) -> Response {
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, kind)], file.data).into_response()
}

/// The job output one event stream follows.
struct Following {
    output: Arc<dyn JobOutput>,
    changed: watch::Receiver<u64>,
    at: usize,
}

/// What one event stream has sent so far.
struct Cursor {
    ui: Ui,
    state: watch::Receiver<BenchState>,
    first: bool,
    /// The state changed since it was last sent.
    stale: bool,
    /// The last state sent was in a UART write: the stream reads no file
    /// then, so nothing the page does adds to the write's load.
    writing: bool,
    /// The job last seen in the state, and its output if the server keeps it.
    job: Option<JobId>,
    following: Option<Following>,
    evidence: Option<EvidenceView>,
    evidence_at: Option<Instant>,
    feeder: Option<FeederView>,
    feeder_at: Option<Instant>,
    pending: VecDeque<Event>,
}

impl Cursor {
    fn new(ui: Ui) -> Self {
        Self {
            state: ui.sources.live.subscribe(),
            ui,
            first: true,
            stale: true,
            writing: false,
            job: None,
            following: None,
            evidence: None,
            evidence_at: None,
            feeder: None,
            feeder_at: None,
            pending: VecDeque::new(),
        }
    }

    /// Wait for a change in the state or the job's output, or the next
    /// tick; `false` once the server's state is gone.
    async fn wait(&mut self) -> bool {
        let Self {
            state,
            stale,
            following,
            ..
        } = self;
        let output = async {
            let woke = match following {
                Some(following) => following.changed.changed().await.is_ok(),
                None => false,
            };
            // An output whose job is gone never wakes again; the state
            // names the next job.
            if !woke {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            // Waking marks the state seen, so the next poll is told.
            changed = state.changed() => {
                *stale = true;
                changed.is_ok()
            }
            () = output => true,
            () = tokio::time::sleep(TICK) => true,
        }
    }

    /// Queue the events due now: a state change, a new job's reset and
    /// its new lines, and the evidence and feeder views when they change.
    async fn poll(&mut self) {
        let first = std::mem::take(&mut self.first);

        if std::mem::take(&mut self.stale) || self.state.has_changed().unwrap_or(false) {
            let mut state = self.state.borrow_and_update().clone();
            state.now = now();
            self.writing = matches!(state.uart, Uart::Writing { .. });
            self.send("state", &state);

            let job = state.job.as_ref().map(|job| job.id);
            if first || job != self.job {
                self.job = job;
                self.follow(job);
            }
        }
        self.read_output();

        // The files are read on their own slower clocks, and not at all
        // during a UART write once the page has its first view.
        let due = |at: Option<Instant>, every: Duration| at.is_none_or(|at| at.elapsed() >= every);
        if first || (!self.writing && due(self.evidence_at, EVIDENCE_EVERY)) {
            self.evidence_at = Some(Instant::now());
            let ui = self.ui.clone();
            let read = tokio::task::spawn_blocking(move || evidence::evidence(&ui.sources));
            if let Ok(view) = read.await
                && self.evidence.as_ref() != Some(&view)
            {
                self.send("evidence", &view);
                self.evidence = Some(view);
            }
        }
        if first || (!self.writing && due(self.feeder_at, FEEDER_EVERY)) {
            self.feeder_at = Some(Instant::now());
            let root = self.ui.sources.root.clone();
            let read = tokio::task::spawn_blocking(move || feeder::feeder(&root));
            if let Ok(view) = read.await
                && self.feeder.as_ref() != Some(&view)
            {
                self.send("feeder", &view);
                self.feeder = Some(view);
            }
        }
    }

    fn send(&mut self, name: &str, data: &impl Serialize) {
        // Every view is plain data and always encodes.
        if let Ok(event) = Event::default().event(name).json_data(data) {
            self.pending.push_back(event);
        }
    }

    /// Start on job `id`'s output: the page drops what it showed, then
    /// gets the newest of what the server kept.
    fn follow(&mut self, id: Option<JobId>) {
        self.pending
            .push_back(Event::default().event("log-reset").data(""));
        self.following = id.and_then(|id| {
            let output = self.ui.sources.live.output(id)?;
            let changed = output.subscribe();
            let (kept, end) = output.read_from(0);
            Some(Following {
                output,
                changed,
                at: end - kept.len().min(LOG_BACKLOG),
            })
        });
    }

    fn read_output(&mut self) {
        let Some(following) = &mut self.following else {
            return;
        };
        // Marked seen before reading, so a line pushed after the read
        // wakes the next wait.
        following.changed.borrow_and_update();
        let (lines, at) = following.output.read_from(following.at);
        following.at = at;
        if !lines.is_empty() {
            let mut text = lines.join("\n");
            text.push('\n');
            self.pending
                .push_back(Event::default().event("log").data(text));
        }
    }
}

async fn events(
    State(ui): State<Ui>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let events = stream::unfold(Cursor::new(ui), |mut cursor| async move {
        loop {
            if cursor.ui.stopping() {
                return None;
            }
            if let Some(event) = cursor.pending.pop_front() {
                return Some((Ok(event), cursor));
            }
            if !cursor.first && !cursor.wait().await {
                return None;
            }
            cursor.poll().await;
        }
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

/// Bind now (so a busy port fails before the server takes jobs), then serve
/// on a background thread until `stop` is cancelled. `flash_bin` is the
/// flash client the page reports by hash; it is never run. `live` is the
/// bench server's state and its jobs' output.
pub fn start(
    root: PathBuf,
    flash_bin: PathBuf,
    listen: &Listen,
    stop: Cancel,
    live: Arc<dyn crate::Live>,
) -> Result<std::thread::JoinHandle<()>> {
    let listener = TcpListener::bind(listen.address)?;
    listener.set_nonblocking(true)?;

    let ui = Ui {
        sources: Sources {
            root,
            flash_bin,
            live,
        },
        flash: Arc::default(),
        stop: stop.clone(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    Ok(std::thread::spawn(move || {
        runtime.block_on(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                eprintln!("The page could not take its listening socket");
                return;
            };

            let shutdown = async move {
                while !stop.is_cancelled() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            };
            if let Err(error) = axum::serve(listener, router(ui))
                .with_graceful_shutdown(shutdown)
                .await
            {
                eprintln!("The page stopped: {error}");
            }
        });
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use futures_util::StreamExt;
    use tower::ServiceExt;

    use crate::Live;
    use crate::views::tests::idle;
    use wellfed_formats::bench::live::{CancelState, JobView};

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
        fn read_from(&self, cursor: usize) -> (Vec<String>, usize) {
            let lines = self.lines.lock().unwrap();
            (lines[cursor.min(lines.len())..].to_vec(), lines.len())
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
            sources: Sources {
                root: tmp.path().into(),
                flash_bin: tmp.path().join("wellfed-flash"),
                live: Arc::clone(&fake) as Arc<dyn Live>,
            },
            flash: Arc::default(),
            stop: Cancel::new(),
        };
        (tmp, fake, router(ui))
    }

    async fn get(router: &Router, uri: &str) -> Response {
        router
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

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
            "/api/feeder",
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

    #[tokio::test]
    async fn the_stream_sends_the_state_then_the_jobs_output_as_it_comes() {
        let (_tmp, fake, router) = ui();
        fake.output.push("before the page connected");
        let response = get(&router, "/api/events").await;
        let mut body = response.into_body().into_data_stream();
        let mut seen = String::new();

        read_until(&mut body, &mut seen, "event: evidence").await;
        assert!(seen.starts_with("event: state\n"), "{seen}");
        assert!(seen.contains("event: log-reset"), "{seen}");

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
}
