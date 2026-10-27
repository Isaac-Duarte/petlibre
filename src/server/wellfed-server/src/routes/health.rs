//! Liveness for anyone, readiness for an owner.

use axum::Json;
use axum::extract::State;
use wellfed_api::{Health, Problem, ServerStatus};

use super::TAG_HEALTH;
use crate::AppState;
use crate::auth::Owner;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Liveness and version, without authentication: the process is up and
/// serving. It reads nothing, so a probe never waits on the database.
#[utoipa::path(
    get,
    path = "/health",
    tag = TAG_HEALTH,
    responses((status = 200, body = Health)),
)]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        version: VERSION.into(),
    })
}

/// Readiness: whether the database answers, each feeder's link and the
/// background jobs' last runs. A failing database is reported in the body,
/// not as an error.
#[utoipa::path(
    get,
    path = "/status",
    tag = TAG_HEALTH,
    responses(
        (status = 200, body = ServerStatus),
        (status = 401, body = Problem),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn status(State(state): State<AppState>, _: Owner) -> Json<ServerStatus> {
    Json(state.service.status(VERSION).await)
}
