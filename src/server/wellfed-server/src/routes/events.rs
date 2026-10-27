//! A feeder's history as the server mirrors it.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum_extra::extract::WithRejection;
use wellfed_api::{EventPage, EventQuery, FeederId, Problem};

use super::{ApiResult, IdPath, TAG_FEEDERS};
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiError;

/// The feeder's history as the server holds it, oldest first.
#[utoipa::path(
    get,
    path = "/feeders/{id}/events",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), EventQuery),
    responses((status = 200, body = EventPage), (status = 404, body = Problem)),
)]
pub(crate) async fn events(
    State(state): State<AppState>,
    _: Auth,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Query(query), _): WithRejection<Query<EventQuery>, ApiError>,
) -> ApiResult<Json<EventPage>> {
    Ok(Json(state.service.events(id, query).await?))
}
