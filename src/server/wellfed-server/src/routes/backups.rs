//! Backups of the server's database in its data directory. Owners only.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use wellfed_api::{Backup, BackupPage, Problem};

use super::{ApiResult, TAG_BACKUPS};
use crate::AppState;
use crate::auth::{Allowed, need};

/// The backups kept, newest first, in one page (`next` is always null).
#[utoipa::path(
    get,
    path = "/backups",
    tag = TAG_BACKUPS,
    responses(
        (status = 200, body = BackupPage),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn backups(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
) -> ApiResult<Json<BackupPage>> {
    Ok(Json(state.service.backups().await?))
}

/// Back the database up now; the oldest backups past `backups.keep` go.
/// Taking one twice is harmless, so it needs no idempotency key.
#[utoipa::path(
    post,
    path = "/backups",
    tag = TAG_BACKUPS,
    responses(
        (status = 201, body = Backup),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn create_backup(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
) -> ApiResult<(StatusCode, Json<Backup>)> {
    Ok((StatusCode::CREATED, Json(state.service.backup().await?)))
}
