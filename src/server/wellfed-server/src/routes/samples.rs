//! A feeder's samples as the server keeps them: raw, or by the hour.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum_extra::extract::WithRejection;
use wellfed_api::{FeederId, Problem, SamplePage, SampleQuery};

use super::{ApiResult, IdPath, TAG_FEEDERS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// One metric's samples, oldest first: each as the feeder sent it with
/// its number (`resolution=raw`, while the owner's raw retention keeps
/// it), or the number's count, minimum, maximum and mean per UTC hour
/// (`resolution=hour`, kept forever).
#[utoipa::path(
    get,
    path = "/feeders/{id}/samples",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), SampleQuery),
    responses(
        (status = 200, body = SamplePage),
        (status = 400, body = Problem, description = "invalid: no metric, a bad span, limit or cursor"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn samples(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Query(query), _): WithRejection<Query<SampleQuery>, ApiError>,
) -> ApiResult<Json<SamplePage>> {
    Ok(Json(state.service.samples(id, query).await?))
}
