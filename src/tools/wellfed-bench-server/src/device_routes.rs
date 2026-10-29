//! The socket's feeder and bank routes:
//! - `POST /device/ask`: one request on the server's session, sent once
//!   (200 with what came of it, 503 `DeviceUnavailable` when nothing was
//!   sent);
//! - `GET /device/pushed?from=&wait=`: the messages the session was pushed,
//!   from number `from`, waiting up to `wait` milliseconds for the first;
//! - `GET /device/session`: whom the session is with (503 while closed);
//! - `GET /banks` and `POST /banks/seed`: the bank ledger.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use wellfed_formats::bench::banks::Ledger;
use wellfed_formats::bench::feeder::{Ask, DeviceUnavailable};
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

async fn ask(State(shared): State<Arc<Shared>>, Json(ask): Json<Ask>) -> Response {
    match shared.feeder().ask(ask.id, ask.op).await {
        Ok(asked) => Json(asked).into_response(),
        Err(reason) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(DeviceUnavailable { reason }),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct From {
    #[serde(default)]
    from: u64,
    /// Milliseconds.
    #[serde(default)]
    wait: u64,
}

async fn pushed(State(shared): State<Arc<Shared>>, Query(from): Query<From>) -> Response {
    let page = shared
        .feeder()
        .pushed()
        .page(from.from, Duration::from_millis(from.wait))
        .await;
    Json(page).into_response()
}

async fn session(State(shared): State<Arc<Shared>>) -> Response {
    if let Some(info) = shared.feeder().info() {
        return Json(info).into_response();
    }
    let reason = match shared.store.read().feeder.session {
        Session::Paused { pause } => Unavailable::Paused { pause },
        Session::Unconfigured { error } => Unavailable::Unconfigured { error },
        Session::Connecting { error, .. } => Unavailable::NotOpen { error },
        Session::Idle | Session::Open { .. } => Unavailable::NotOpen { error: None },
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(DeviceUnavailable { reason }),
    )
        .into_response()
}

async fn banks(State(shared): State<Arc<Shared>>) -> Json<Ledger> {
    Json(shared.banks.read())
}

async fn seed(State(shared): State<Arc<Shared>>, Json(seed): Json<Seed>) -> Response {
    let banks = shared.banks.clone();
    let seeded = tokio::task::spawn_blocking(move || banks.seed(&seed)).await;
    match seeded {
        Ok(Ok(seeded)) => (StatusCode::CREATED, Json(seeded)).into_response(),
        Ok(Err(error)) => Refusal::Seed { error }.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
