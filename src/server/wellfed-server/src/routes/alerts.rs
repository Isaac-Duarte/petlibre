//! Alerts: everyone reads them; an owner acknowledges one. The server
//! raises and clears them on its own, and the stream carries each change.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum_extra::extract::WithRejection;
use wellfed_api::{Alert, AlertId, AlertPage, AlertQuery, Problem};

use super::{ApiResult, TAG_ALERTS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// The alerts, oldest first; `state=open` lists those nobody has
/// acknowledged and that still hold.
#[utoipa::path(
    get,
    path = "/alerts",
    tag = TAG_ALERTS,
    params(AlertQuery),
    responses(
        (status = 200, body = AlertPage),
        (status = 400, body = Problem, description = "invalid: a bad state, feeder, limit or cursor"),
        (status = 401, body = Problem),
    ),
)]
pub(crate) async fn alerts(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Query(query), _): WithRejection<Query<AlertQuery>, ApiError>,
) -> ApiResult<Json<AlertPage>> {
    Ok(Json(state.service.alerts(query).await?))
}

/// Acknowledge an alert: an owner has seen it. It still clears when its
/// condition ends. One acknowledged before is answered as it stands.
#[utoipa::path(
    post,
    path = "/alerts/{id}/ack",
    tag = TAG_ALERTS,
    params(("id" = AlertId, Path)),
    responses(
        (status = 200, body = Alert),
        (status = 401, body = Problem),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn acknowledge(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::AcknowledgeAlerts>,
    WithRejection(Path(id), _): WithRejection<Path<AlertId>, ApiError>,
) -> ApiResult<Json<Alert>> {
    Ok(Json(
        state.service.acknowledge_alert(&caller.user, id).await?,
    ))
}
