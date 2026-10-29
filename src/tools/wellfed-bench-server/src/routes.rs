//! The socket's routes:
//! - `GET /state`: the live state;
//! - `GET /events`: the live state again on every change (SSE `state`);
//! - `POST /jobs`: parse a command line and start it as a job (201), or
//!   refuse it (400 for no job, 409 while one runs);
//! - `GET /jobs/{id}/stream`: the job's output from its first line, then its
//!   end, as newline-delimited JSON; a client that disconnects only stops
//!   reading;
//! - `POST /jobs/{id}/cancel`: stop the running job.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use futures_util::stream::{self, Stream};
use tokio::sync::watch;
use wellfed_formats::bench::live::{BenchState, JobEvent, JobId, Phase};
use wellfed_formats::bench::now;

use crate::api::{Accepted, JobRequest, Refusal};

use crate::server::Shared;
use crate::state::JobLog;

pub(crate) fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/state", get(state))
        .route("/events", get(events))
        .route("/jobs", post(submit))
        .route("/jobs/{id}/stream", get(follow))
        .route("/jobs/{id}/cancel", post(cancel))
        .with_state(shared)
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Busy { .. } | Self::Finished { .. } => StatusCode::CONFLICT,
            Self::Stopping => StatusCode::SERVICE_UNAVAILABLE,
            Self::NoSuchJob { .. } => StatusCode::NOT_FOUND,
            Self::Unparsed { .. }
            | Self::NotAJob { .. }
            | Self::OtherCheckout { .. }
            | Self::BadName { .. }
            | Self::BadLength { .. } => StatusCode::BAD_REQUEST,
        };
        (status, Json(self)).into_response()
    }
}

async fn state(State(shared): State<Arc<Shared>>) -> Json<BenchState> {
    Json(shared.store.read())
}

async fn submit(State(shared): State<Arc<Shared>>, Json(request): Json<JobRequest>) -> Response {
    match shared.start(&request) {
        Ok(id) => (StatusCode::CREATED, Json(Accepted { id })).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn cancel(State(shared): State<Arc<Shared>>, Path(id): Path<JobId>) -> Response {
    match shared.cancel(id) {
        Ok(()) => (StatusCode::ACCEPTED, Json(Accepted { id })).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// The state now, then again on each change. The stream ends once it has
/// sent a stopping server's state, so it never holds a quitting server.
async fn events(
    State(shared): State<Arc<Shared>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    struct Cursor {
        receiver: watch::Receiver<BenchState>,
        first: bool,
        done: bool,
    }

    let cursor = Cursor {
        receiver: shared.store.subscribe(),
        first: true,
        done: false,
    };
    let events = stream::unfold(cursor, |mut cursor| async move {
        if cursor.done || (!cursor.first && cursor.receiver.changed().await.is_err()) {
            return None;
        }
        cursor.first = false;

        let mut state = cursor.receiver.borrow_and_update().clone();
        state.now = now();
        cursor.done = state.server.phase == Phase::Stopping;
        let event = Event::default()
            .event("state")
            .json_data(&state)
            .unwrap_or_else(|_| Event::default().event("state"));
        Some((Ok(event), cursor))
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

async fn follow(State(shared): State<Arc<Shared>>, Path(id): Path<JobId>) -> Response {
    let Some(log) = shared.log(id) else {
        return Refusal::NoSuchJob { id }.into_response();
    };
    (
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(lines(log)),
    )
        .into_response()
}

/// A job's events from its first kept line: whatever is there, then each
/// line as it comes, then its end.
fn lines(log: Arc<JobLog>) -> impl Stream<Item = Result<Bytes, Infallible>> {
    struct Cursor {
        changed: watch::Receiver<u64>,
        log: Arc<JobLog>,
        at: usize,
        done: bool,
    }

    let cursor = Cursor {
        changed: log.subscribe(),
        log,
        at: 0,
        done: false,
    };
    stream::unfold(cursor, |mut cursor| async move {
        loop {
            if cursor.done {
                return None;
            }
            // Marked seen before reading, so a line pushed after the read
            // wakes the wait below.
            cursor.changed.borrow_and_update();
            let unread = cursor.log.read_from(cursor.at);
            cursor.at = unread.cursor;

            let mut events: Vec<JobEvent> = unread.lines.into_iter().map(JobEvent::Line).collect();
            if let Some(end) = unread.end {
                events.push(JobEvent::End(end));
                cursor.done = true;
            }
            if !events.is_empty() {
                return Some((Ok(encode(&events)), cursor));
            }
            if cursor.changed.changed().await.is_err() {
                return None;
            }
        }
    })
}

fn encode(events: &[JobEvent]) -> Bytes {
    let mut out = Vec::new();
    for event in events {
        // A line and an end are plain data and always encode.
        if serde_json::to_writer(&mut out, event).is_ok() {
            out.push(b'\n');
        }
    }
    Bytes::from(out)
}
