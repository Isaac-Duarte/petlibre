//! Bearer tokens for phones and the CLI: each user manages their own, and
//! an owner anyone's.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use wellfed_api::{CreatedToken, NewToken, Problem, Token, TokenId};

use super::{ApiResult, JsonBody, TAG_TOKENS, insert};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// The caller's own tokens, or everyone's for an owner, oldest first. A
/// token's secret is never listed.
#[utoipa::path(
    get,
    path = "/tokens",
    tag = TAG_TOKENS,
    responses(
        (status = 200, body = Vec<Token>),
        (status = 401, body = Problem),
    ),
)]
pub(crate) async fn tokens(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnAccount>,
) -> ApiResult<Json<Vec<Token>>> {
    Ok(Json(state.service.tokens(&caller.user).await?))
}

/// Make a token for the logged-in user's device. The answer holds its
/// secret, the only time the server shows it; the token lasts until
/// revoked, or until its expiry if it has one.
#[utoipa::path(
    post,
    path = "/tokens",
    tag = TAG_TOKENS,
    request_body = NewToken,
    responses(
        (status = 201, body = CreatedToken, headers(("Cache-Control" = String, description = "no-store"))),
        (status = 400, body = Problem, description = "invalid: a name, or an expiry already past"),
        (status = 401, body = Problem),
        (status = 403, body = Problem, description = "session_required: a token cannot make another"),
    ),
)]
pub(crate) async fn create_token(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnAccount>,
    WithRejection(Json(new), _): JsonBody<NewToken>,
) -> ApiResult<Response> {
    let created = state.service.create_token(&caller, new).await?;

    // The secret must not linger in a cache.
    let mut response = (StatusCode::CREATED, Json(created)).into_response();
    insert(&mut response, header::CACHE_CONTROL, "no-store");
    Ok(response)
}

/// Revoke a token: the caller's own, or anyone's for an owner. Another
/// user's is not found.
#[utoipa::path(
    delete,
    path = "/tokens/{id}",
    tag = TAG_TOKENS,
    params(("id" = TokenId, Path)),
    responses(
        (status = 204),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn revoke_token(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnAccount>,
    WithRejection(Path(id), _): WithRejection<Path<TokenId>, ApiError>,
) -> ApiResult<StatusCode> {
    state.service.revoke_token(&caller, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
