//! The audit log: each user reads their own entries, an owner everyone's.

use axum::Json;
use axum::extract::{Query, State};
use axum_extra::extract::WithRejection;
use wellfed_api::{AuditPage, PageQuery, Problem};

use super::{ApiResult, TAG_AUDIT};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// Who did what through the server, oldest first: every write sent to a
/// feeder (with its state and outcome as they stand) and every account
/// change, logins and failed logins included. The caller's own entries,
/// or everyone's for an owner.
#[utoipa::path(
    get,
    path = "/audit",
    tag = TAG_AUDIT,
    params(PageQuery),
    responses(
        (status = 200, body = AuditPage),
        (status = 400, body = Problem, description = "invalid: a bad limit or cursor"),
        (status = 401, body = Problem),
    ),
)]
pub(crate) async fn audit(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnHistory>,
    WithRejection(Query(query), _): WithRejection<Query<PageQuery>, ApiError>,
) -> ApiResult<Json<AuditPage>> {
    Ok(Json(state.service.audit(&caller.user, query).await?))
}
