//! The household's accounts, which owners manage.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum_extra::extract::WithRejection;
use wellfed_api::{NewUser, Problem, User, UserId, UserPatch};

use super::{ApiResult, JsonBody, TAG_USERS};
use crate::AppState;
use crate::auth::{self, Allowed, need};
use crate::error::ApiError;

type UserPath = WithRejection<Path<UserId>, ApiError>;

/// Every account, by name.
#[utoipa::path(
    get,
    path = "/users",
    tag = TAG_USERS,
    responses(
        (status = 200, body = Vec<User>),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn users(
    State(state): State<AppState>,
    _: Allowed<need::Users>,
) -> ApiResult<Json<Vec<User>>> {
    Ok(Json(state.service.users().await?))
}

/// Add an account, with a temporary password the owner passes on. Only
/// from a login, never a token.
#[utoipa::path(
    post,
    path = "/users",
    tag = TAG_USERS,
    request_body = NewUser,
    responses(
        (status = 201, body = User),
        (status = 400, body = Problem, description = "invalid: a name, a password under 10 characters, or an owner with an expiry"),
        (status = 403, body = Problem, description = "forbidden: owners only; session_required: not from a token"),
        (status = 409, body = Problem, description = "name_taken"),
    ),
)]
pub(crate) async fn create_user(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::Users>,
    WithRejection(Json(request), _): JsonBody<NewUser>,
) -> ApiResult<(StatusCode, Json<User>)> {
    let user = state.service.create_user(&caller, request).await?;
    Ok((StatusCode::CREATED, Json(user)))
}

/// Change an account's role, expiry or password; a password only from a
/// login, never a token. A role applies to the
/// user's tokens at once. Setting a password ends that user's sessions (an
/// owner's own, all but the one asking) and leaves their tokens; extending
/// a disabled account enables it, and its tokens with it.
#[utoipa::path(
    patch,
    path = "/users/{id}",
    tag = TAG_USERS,
    params(("id" = UserId, Path)),
    request_body = UserPatch,
    responses(
        (status = 200, body = User),
        (status = 400, body = Problem, description = "invalid: a password under 10 characters, or an owner with an expiry"),
        (status = 403, body = Problem, description = "forbidden: owners only; session_required: a password from a token"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "last_owner: the household's only owner keeps the role"),
    ),
)]
pub(crate) async fn update_user(
    State(state): State<AppState>,
    Allowed(actor, _): Allowed<need::Users>,
    headers: HeaderMap,
    WithRejection(Path(id), _): UserPath,
    WithRejection(Json(patch), _): JsonBody<UserPatch>,
) -> ApiResult<Json<User>> {
    let session = auth::session(&headers);
    let user = state
        .service
        .update_user(&actor, session.as_ref(), id, patch)
        .await?;
    Ok(Json(user))
}

/// Delete an account, ending its sessions and revoking its tokens. Its
/// phones' feeder keys are revoked separately.
#[utoipa::path(
    delete,
    path = "/users/{id}",
    tag = TAG_USERS,
    params(("id" = UserId, Path)),
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "last_owner"),
    ),
)]
pub(crate) async fn delete_user(
    State(state): State<AppState>,
    _: Allowed<need::Users>,
    WithRejection(Path(id), _): UserPath,
) -> ApiResult<StatusCode> {
    state.service.delete_user(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
