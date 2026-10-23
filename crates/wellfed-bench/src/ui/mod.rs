//! `--ui`: a read-only page of bench progress, sessions and health.
//!
//! It reads the status files and evidence directories only. It never opens a
//! serial port, so it cannot disturb an update, a dump or a capture, and it
//! never writes. The page shows private UART output, so it binds loopback by
//! default. `--listen` serves one named interface instead, such as the
//! Tailscale address; wildcard addresses are refused.

mod views;

use std::convert::Infallible;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

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

use crate::config::STATUS_DIR;
use crate::status::{self, LOG};
use crate::{Error, Result, interrupted};
use views::{FlashCache, Snapshot, Sources};

#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

/// Live log text is sent in chunks of at most this much.
const LOG_CHUNK: u64 = 256 << 10;
/// A newly connected page starts this far from the end of the log.
const LOG_BACKLOG: u64 = 512 << 10;

#[derive(Clone)]
struct Ui {
    sources: Sources,
    flash: Arc<Mutex<FlashCache>>,
}

/// Where the page listens.
pub struct Listen {
    address: SocketAddr,
}

impl Listen {
    pub fn new(listen: &str, port: u16) -> Result<Self> {
        let ip: IpAddr = listen
            .parse()
            .map_err(|_| Error::Bench("--listen needs a literal IP address".into()))?;
        if ip.is_unspecified() {
            return Err(Error::Bench(
                "Refusing a wildcard address; name the interface address to serve on".into(),
            ));
        }
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
        .route("/api/status", get(status))
        .route("/api/events", get(events))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/{name}", get(session))
        .route("/api/health", get(health))
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

async fn status(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(views::snapshot(&ui.sources))).await
}

async fn sessions(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| Some(views::sessions(&ui.sources))).await
}

async fn session(State(ui): State<Ui>, UrlPath(name): UrlPath<String>) -> Response {
    view(&ui, move |ui| views::session(&ui.sources, &name)).await
}

async fn health(State(ui): State<Ui>) -> Response {
    view(&ui, |ui| {
        // The cache holds only a hash; one left mid-update by a panicked
        // view is recomputed when the binary next changes, so take it as is.
        let mut flash = ui.flash.lock().unwrap_or_else(PoisonError::into_inner);
        Some(views::health(&ui.sources, &mut flash))
    })
    .await
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
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, kind)], file.data).into_response()
}

/// What one event stream has sent so far.
struct Cursor {
    ui: Ui,
    /// The last status sent; `None` before the first.
    snapshot: Option<Snapshot>,
    started: Option<f64>,
    offset: u64,
}

impl Cursor {
    /// The events due now: a status change, a log reset, new log text.
    fn poll(&mut self) -> Vec<Event> {
        let directory = self.ui.sources.root.join(STATUS_DIR);
        let first = self.snapshot.is_none();
        let mut out = Vec::new();

        let snapshot = views::snapshot(&self.ui.sources);
        let changed = self
            .snapshot
            .as_ref()
            .is_none_or(|sent| !snapshot.same_as(sent));
        if changed && let Ok(text) = serde_json::to_string(&snapshot) {
            out.push(Event::default().event("status").data(text));
            self.snapshot = Some(snapshot);
        }

        // A new command, or a log that shrank, restarts the page's log view
        // near the end of the current file.
        let started = status::current(&directory).map(|c| c.started);
        let log = directory.join(LOG);
        let size = fs::metadata(&log).map_or(0, |m| m.len());
        if first || started != self.started || size < self.offset {
            self.started = started;
            self.offset = size.saturating_sub(LOG_BACKLOG);
            out.push(Event::default().event("log-reset").data(""));
        }

        if size > self.offset {
            let mut chunk = Vec::new();
            if let Ok(mut file) = fs::File::open(&log)
                && file.seek(SeekFrom::Start(self.offset)).is_ok()
            {
                let _ = file.take(LOG_CHUNK).read_to_end(&mut chunk);
            }
            // Send whole lines; a partial one waits for its newline.
            let whole = chunk.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            if whole > 0 {
                self.offset += whole as u64;
                let text = String::from_utf8_lossy(&chunk[..whole]).into_owned();
                out.push(Event::default().event("log").data(text));
            }
        }

        out
    }
}

async fn events(
    State(ui): State<Ui>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let cursor = Cursor {
        ui,
        snapshot: None,
        started: None,
        offset: 0,
    };

    let ticks = stream::unfold(Some(cursor), |cursor| async move {
        let mut cursor = cursor?;
        loop {
            if interrupted() {
                return None;
            }
            let (back, events) = tokio::task::spawn_blocking(move || {
                let events = cursor.poll();
                (cursor, events)
            })
            .await
            .ok()?;
            cursor = back;
            if !events.is_empty() {
                return Some((stream::iter(events.into_iter().map(Ok)), Some(cursor)));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    use futures_util::StreamExt;
    Sse::new(ticks.flatten()).keep_alive(KeepAlive::default())
}

/// Bind now (so a busy port fails before any bench command), then serve on a
/// background thread until the operator interrupts.
pub fn start(
    root: PathBuf,
    flash_bin: PathBuf,
    listen: &Listen,
) -> Result<std::thread::JoinHandle<()>> {
    let listener = TcpListener::bind(listen.address)?;
    listener.set_nonblocking(true)?;

    let ui = Ui {
        sources: Sources { root, flash_bin },
        flash: Arc::default(),
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

            let shutdown = async {
                while !interrupted() {
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
    use tower::ServiceExt;

    fn ui() -> (tempfile::TempDir, Router) {
        let tmp = tempfile::tempdir().unwrap();
        let ui = Ui {
            sources: Sources {
                root: tmp.path().into(),
                flash_bin: tmp.path().join("wellfed-flash"),
            },
            flash: Arc::default(),
        };
        (tmp, router(ui))
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
        let (_tmp, router) = ui();
        for (uri, kind) in [
            ("/", "text/html; charset=utf-8"),
            ("/app.js", "text/javascript; charset=utf-8"),
            ("/style.css", "text/css; charset=utf-8"),
        ] {
            let response = get(&router, uri).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], kind);
        }

        for uri in ["/api/status", "/api/sessions", "/api/health"] {
            assert_eq!(get(&router, uri).await.status(), StatusCode::OK, "{uri}");
        }

        assert_eq!(
            get(&router, "/missing").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&router, "/api/sessions/absent").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn wildcard_listen_is_refused() {
        assert!(Listen::new("0.0.0.0", 8780).is_err());
        assert!(Listen::new("::", 8780).is_err());
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
