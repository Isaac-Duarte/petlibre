//! Installs: an owner installs one of the server's releases on a feeder,
//! one install at a time per server, and anyone reads where it stands.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum_extra::extract::WithRejection;
use wellfed_api::{FeederId, Install, InstallRequest, Problem};

use super::{ApiResult, IdPath, JsonBody, TAG_FEEDERS};
use crate::AppState;
use crate::auth::{Allowed, need};

/// Install a release on the feeder. It runs in the background: this answers
/// once it has started, and the event stream carries its progress and its
/// end as `install` updates. Nothing is offered again on the owner's
/// behalf; another install is another request.
#[utoipa::path(
    post,
    path = "/feeders/{id}/install",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    request_body = InstallRequest,
    responses(
        (status = 202, body = Install, description = "started"),
        (status = 400, body = Problem, description = "invalid: a malformed body"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: no such feeder or release"),
        (status = 409, body = Problem, description = "install_running: another install is running; feeder_refused (`forbidden`): the server's key is not an owner's on this feeder; stock_backup_needed: the feeder runs from bank 0 with stock in bank 1 and no verified backup is kept"),
        (status = 422, body = Problem, description = "release_refused: the release no longer verifies"),
        (status = 503, body = Problem, description = "feeder_offline"),
    ),
)]
pub(crate) async fn install(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::ServerData>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Json(request), _): JsonBody<InstallRequest>,
) -> ApiResult<(StatusCode, Json<Install>)> {
    let install = state.service.install(&caller, id, request).await?;
    Ok((StatusCode::ACCEPTED, Json(install)))
}

/// The feeder's running install, or else its last one.
#[utoipa::path(
    get,
    path = "/feeders/{id}/install",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, body = Install),
        (status = 404, body = Problem, description = "not_found: no such feeder, or no install has run on it"),
    ),
)]
pub(crate) async fn install_state(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Json<Install>> {
    Ok(Json(state.service.install_state(id).await?))
}
