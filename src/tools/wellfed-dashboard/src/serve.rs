//! The page's HTTP server: the built web app from `src/web/apps/bench/dist`,
//! embedded at compile time, the JSON views it reads, and one event stream
//! of the server's live state, the running job's output and the views that
//! change with it.
//!
//! The page shows private UART output and job output, so it binds loopback
//! by default and answers only requests addressed to it by IP or as
//! `localhost` (see [`Hosts`]). `--listen` serves another address: a named
//! interface such as the Tailscale address, or a wildcard for every
//! interface when the owner chooses to.
//!
//! One route writes: `POST /api/jobs/{id}/answer`, the answer to the
//! question a job asks, which the server takes only for that question and
//! of its kind. It takes JSON alone, so a page elsewhere cannot send it
//! without a CORS preflight this server never grants.
//!
//! While the flash client holds the UART (writing, or in a loader session),
//! nothing here reads a file: the views answer [`Deferred`] and the stream
//! sends only the live state and the job's output, which are in memory.

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
use axum::routing::{get, post};
use futures_util::stream::{self, Stream};
use rust_embed::Embed;
use serde::Serialize;
use tokio::sync::watch;

use super::runs;
use super::stock::{self, StockView};
use super::views::evidence::{self, EvidenceView};
use super::views::health::{self, FlashCache};
use super::views::{Sources, sessions};

use wellfed_formats::bench::live::{AnswerError, AnswerRequest, BenchState, JobId};
use wellfed_formats::bench::{Cancel, now};

use crate::{Error, JobOutput, Result};

#[derive(Embed)]
// Relative to this crate; `make ui` builds it.
#[folder = "../../../src/web/apps/bench/dist/"]
struct Assets;

/// A newly connected page gets at most this many of the job's lines, the
/// most the page keeps.
const LOG_BACKLOG: usize = 20_000;
/// The most lines one read copies under the job's lock.
const LOG_BATCH: usize = 2_000;
/// How often the evidence files are read again for a connected page.
const EVIDENCE_EVERY: Duration = Duration::from_secs(1);
/// How often the event stream asks the local stock server about the feeder.
const STOCK_EVERY: Duration = Duration::from_secs(5);
/// How often a quiet stream looks at the clocks above and the stop flag.
const TICK: Duration = Duration::from_millis(250);
/// When a page should ask again for a view it was refused during a write.
const DEFERRED_RETRY: &str = "5";

#[derive(Clone)]
struct Ui {
    sources: Sources,
    flash: Arc<Mutex<FlashCache>>,
    hosts: Hosts,
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

/// The `Host` names the page answers. A page elsewhere that resolves its
/// own name to this host (DNS rebinding) sends that name, so only
/// `localhost` and IP literals are taken: the bound address and loopback,
/// or any literal under a wildcard bind, always with the bound port.
#[derive(Debug, Clone, Copy)]
struct Hosts {
    ip: IpAddr,
    port: u16,
}

impl Hosts {
    fn allows(self, host: &str) -> bool {
        let (name, port) = match host.strip_prefix('[') {
            Some(rest) => match rest.split_once(']') {
                Some((name, port)) => (name, port),
                None => return false,
            },
            None => host.rfind(':').map_or((host, ""), |at| host.split_at(at)),
        };
        let port = match port.strip_prefix(':') {
            Some(port) => port.parse::<u16>().ok(),
            // Inside brackets, or a bare name: no port given.
            None if port.is_empty() => Some(80),
            None => return false,
        };
        if port != Some(self.port) {
            return false;
        }

        if name.eq_ignore_ascii_case("localhost") {
            return true;
        }
        match name.parse::<IpAddr>() {
            Ok(ip) => ip == self.ip || ip.is_loopback() || self.ip.is_unspecified(),
            Err(_) => false,
        }
    }
}

/// The answer to a view that would read files while the flash client
/// holds the UART: 503 with this body and a `Retry-After`. The page reads
/// any refusal as "keep what is shown", so it never parses this.
#[derive(Debug, Serialize)]
struct Deferred {
    deferred: DeferredBy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum DeferredBy {
    /// The flash client holds the UART.
    Uart,
}

fn deferred() -> Response {
    let body = Deferred {
        deferred: DeferredBy::Uart,
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, DEFERRED_RETRY)],
        Json(body),
    )
        .into_response()
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
        .route("/api/stock", get(stock_status))
        .route("/api/jobs/{id}/answer", post(answer))
        .fallback(asset)
        .layer(middleware::from_fn(no_store))
        .layer(middleware::from_fn_with_state(ui.hosts, addressed))
        .with_state(ui)
}

/// Refuse a request whose `Host` is not this page's (421).
async fn addressed(State(hosts): State<Hosts>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    if !host.is_some_and(|host| hosts.allows(host)) {
        return StatusCode::MISDIRECTED_REQUEST.into_response();
    }
    next.run(request).await
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Run a view that reads files on the blocking pool, unless the flash
/// client holds the UART; `None` is a missing page.
async fn view<T: Serialize + Send + 'static>(
    ui: &Ui,
    make: impl FnOnce(&Ui) -> Option<T> + Send + 'static,
) -> Response {
    if ui.sources.uart_busy() {
        return deferred();
    }
    let ui = ui.clone();
    match tokio::task::spawn_blocking(move || make(&ui)).await {
        Ok(Some(view)) => Json(view).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn state(State(ui): State<Ui>) -> Json<BenchState> {
    let mut state = ui.sources.state();
    state.now = now();
    Json(state)
}

/// 204 once the server took the answer; otherwise the refusal, 409 for a
/// question that is no longer (or not yet) the one asked.
async fn answer(
    State(ui): State<Ui>,
    UrlPath(id): UrlPath<JobId>,
    Json(request): Json<AnswerRequest>,
) -> Response {
    match ui.sources.live.answer(id, request.seq, request.answer) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            let status = match error {
                AnswerError::Mismatched { .. } => StatusCode::BAD_REQUEST,
                AnswerError::NotRunning { .. }
                | AnswerError::NoPrompt { .. }
                | AnswerError::WrongSeq { .. } => StatusCode::CONFLICT,
            };
            (status, Json(error)).into_response()
        }
    }
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

async fn stock_status(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(stock::stock(&ui.sources.root))).await
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
    /// The flash client held the UART in the last state sent: the stream
    /// reads no file and asks no server until it lets go.
    busy: bool,
    /// The job last seen in the state, and its output if the server keeps it.
    job: Option<JobId>,
    following: Option<Following>,
    /// The last read stopped at its batch limit: read again before waiting.
    more: bool,
    evidence: Option<EvidenceView>,
    evidence_at: Option<Instant>,
    stock: Option<StockView>,
    stock_at: Option<Instant>,
    pending: VecDeque<Event>,
}

impl Cursor {
    fn new(ui: Ui) -> Self {
        Self {
            state: ui.sources.live.subscribe(),
            ui,
            first: true,
            stale: true,
            busy: false,
            job: None,
            following: None,
            more: false,
            evidence: None,
            evidence_at: None,
            stock: None,
            stock_at: None,
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
    /// its new lines, and the evidence and stock views when they change.
    async fn poll(&mut self) {
        let first = std::mem::take(&mut self.first);

        // The state comes first, so the gate below knows whether the flash
        // client holds the UART before any file is read.
        if std::mem::take(&mut self.stale) || self.state.has_changed().unwrap_or(false) {
            let mut state = self.state.borrow_and_update().clone();
            state.now = now();
            self.busy = Sources::holds_uart(&state);
            self.send("state", &state);

            let job = state.job.as_ref().map(|job| job.id);
            if first || job != self.job {
                self.job = job;
                self.follow(job);
            }
        }
        self.read_output();
        if self.busy {
            return;
        }

        let due = |at: Option<Instant>, every: Duration| at.is_none_or(|at| at.elapsed() >= every);
        if due(self.evidence_at, EVIDENCE_EVERY) {
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
        if due(self.stock_at, STOCK_EVERY) {
            self.stock_at = Some(Instant::now());
            let root = self.ui.sources.root.clone();
            let read = tokio::task::spawn_blocking(move || stock::stock(&root));
            if let Ok(view) = read.await
                && self.stock.as_ref() != Some(&view)
            {
                self.send("stock", &view);
                self.stock = Some(view);
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
        // A browser drops an event with no data, so the reset names the job
        // that follows (or `none`).
        let next = id.map_or_else(|| "none".to_string(), |id| id.to_string());
        self.pending
            .push_back(Event::default().event("log-reset").data(next));
        self.following = id.and_then(|id| {
            let output = self.ui.sources.live.output(id)?;
            let changed = output.subscribe();
            let at = output.end().saturating_sub(LOG_BACKLOG);
            Some(Following {
                output,
                changed,
                at,
            })
        });
    }

    fn read_output(&mut self) {
        self.more = false;
        let Some(following) = &mut self.following else {
            return;
        };
        // Marked seen before reading, so a line pushed after the read
        // wakes the next wait.
        following.changed.borrow_and_update();
        let (lines, at) = following.output.read_from(following.at, LOG_BATCH);
        following.at = at;
        self.more = lines.len() == LOG_BATCH;
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
            if !cursor.first && !cursor.more && !cursor.wait().await {
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
        sources: Sources::new(root, flash_bin, live),
        flash: Arc::default(),
        hosts: Hosts {
            ip: listen.address.ip(),
            port: listen.address.port(),
        },
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
mod tests;
