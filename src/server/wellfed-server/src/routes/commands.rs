//! The command journal: the writes sent through the server, read by key
//! or listed. Each user reads their own; an owner reads everyone's.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum_extra::extract::WithRejection;
use uuid::Uuid;
use wellfed_api::{Command, CommandPage, CommandQuery, Problem};

use super::{ApiResult, TAG_COMMANDS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// The journal, oldest first: the caller's own writes, or everyone's for
/// an owner. `state=unknown` lists the writes whose outcome is uncertain.
#[utoipa::path(
    get,
    path = "/commands",
    tag = TAG_COMMANDS,
    params(CommandQuery),
    responses(
        (status = 200, body = CommandPage),
        (status = 400, body = Problem, description = "invalid: a bad limit or cursor"),
        (status = 401, body = Problem),
    ),
)]
pub(crate) async fn commands(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnHistory>,
    WithRejection(Query(query), _): WithRejection<Query<CommandQuery>, ApiError>,
) -> ApiResult<Json<CommandPage>> {
    Ok(Json(state.service.commands(&caller.user, query).await?))
}

/// One write, by the `Idempotency-Key` it was sent under. Another user's
/// is not found, unless the caller is an owner.
#[utoipa::path(
    get,
    path = "/commands/{key}",
    tag = TAG_COMMANDS,
    params(("key" = Uuid, Path)),
    responses(
        (status = 200, body = Command),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn command(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnHistory>,
    WithRejection(Path(key), _): WithRejection<Path<Uuid>, ApiError>,
) -> ApiResult<Json<Command>> {
    Ok(Json(
        state
            .service
            .command(&caller.user, key.into_bytes())
            .await?,
    ))
}
