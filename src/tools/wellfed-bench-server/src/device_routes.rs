//! The socket's feeder and bank routes:
//! - `POST /device/ask`: one request on the server's session (200 with what
//!   came of it; 503 `DeviceUnavailable` when nothing was sent, 409 for an
//!   id already out);
//! - `GET /device/pushed?stream=&from=&wait=`: the history (`events`) or
//!   the debug log (`logs`) the session was pushed, from number `from`,
//!   waiting up to `wait` milliseconds for the first;
//! - `GET /device/session`: whom the session speaks as and with, and the
//!   feeder's welcome once open (503 while paused or unconfigured);
//! - `GET /banks` and `POST /banks/seed`: the bank ledger; a seed is
//!   refused while a job runs.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use wellfed_formats::bench::banks::Ledger;
use wellfed_formats::bench::feeder::{Ask, DeviceUnavailable, Stream};
use wellfed_formats::bench::live::{Session, Unavailable};

use crate::api::{Refusal, Seed};
use crate::server::Shared;

pub(crate) fn routes() -> Router<Arc<Shared>> {
    Router::new()
        .route("/device/ask", post(ask))
        .route("/device/pushed", get(pushed))
        .route("/device/session", get(session))
        .route("/banks", get(banks))
        .route("/banks/seed", post(seed))
}

fn unavailable(reason: Unavailable) -> Response {
    let status = match reason {
        Unavailable::DuplicateId { .. } => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(DeviceUnavailable { reason })).into_response()
}

async fn ask(State(shared): State<Arc<Shared>>, Json(ask): Json<Ask>) -> Response {
    match shared.feeder().ask(ask.id, ask.op).await {
        Ok(asked) => Json(asked).into_response(),
        Err(reason) => unavailable(reason),
    }
}

#[derive(Deserialize)]
struct Page {
    stream: Stream,
    #[serde(default)]
    from: u64,
    /// Milliseconds.
    #[serde(default)]
    wait: u64,
}

async fn pushed(State(shared): State<Arc<Shared>>, Query(page): Query<Page>) -> Response {
    let page = shared
        .feeder()
        .pushed(page.stream)
        .page(page.from, Duration::from_millis(page.wait))
        .await;
    Json(page).into_response()
}

/// Who the session is, whenever it is configured and not paused: a client
/// that finds it still opening waits for it rather than giving up.
async fn session(State(shared): State<Arc<Shared>>) -> Response {
    let reason = match shared.store.read().feeder.session {
        Session::Paused { pause } => Unavailable::Paused { pause },
        Session::Unconfigured { error } => Unavailable::Unconfigured { error },
        Session::Idle | Session::Connecting { .. } | Session::Open { .. } => {
            match shared.feeder().info() {
                Some(info) => return Json(info).into_response(),
                None => Unavailable::Ended,
            }
        }
    };
    unavailable(reason)
}

async fn banks(State(shared): State<Arc<Shared>>) -> Json<Ledger> {
    Json(shared.banks.read())
}

async fn seed(State(shared): State<Arc<Shared>>, Json(seed): Json<Seed>) -> Response {
    let held = Arc::clone(&shared);
    let seeded =
        tokio::task::spawn_blocking(move || held.while_idle(|| held.banks.seed(&seed))).await;
    match seeded {
        Ok(Ok(Ok(seeded))) => (StatusCode::CREATED, Json(seeded)).into_response(),
        Ok(Ok(Err(error))) => Refusal::Seed { error }.into_response(),
        Ok(Err(refusal)) => refusal.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
