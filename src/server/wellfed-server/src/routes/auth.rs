//! First-owner setup and the session: logging in and out.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use wellfed_api::{Credentials, Problem, SetupState, User};

use super::{ApiResult, JsonBody, TAG_AUTH, insert};
use crate::AppState;
use crate::auth::{self, Auth};

/// Whether a fresh server is waiting for its first owner.
#[utoipa::path(get, path = "/setup", tag = TAG_AUTH, responses((status = 200, body = SetupState)))]
pub(crate) async fn setup_state(State(state): State<AppState>) -> ApiResult<Json<SetupState>> {
    let open = state.service.setup_open().await?;
    Ok(Json(SetupState { open }))
}

/// Create the first owner and log them in. Closed once any account exists.
#[utoipa::path(
    post,
    path = "/setup",
    tag = TAG_AUTH,
    request_body = Credentials,
    responses(
        (status = 201, body = User, description = "Created; the session cookie is set"),
        (status = 409, body = Problem, description = "setup_closed"),
    ),
)]
pub(crate) async fn setup(
    State(state): State<AppState>,
    WithRejection(Json(credentials), _): JsonBody<Credentials>,
) -> ApiResult<Response> {
    let (user, token) = state.service.setup(credentials).await?;
    let cookie = auth::session_cookie(&token, state.service.session_lifetime());
    Ok(with_cookie(StatusCode::CREATED, &cookie, user))
}

/// The logged-in user.
#[utoipa::path(
    get,
    path = "/session",
    tag = TAG_AUTH,
    responses((status = 200, body = User), (status = 401, body = Problem)),
)]
pub(crate) async fn session(Auth(user): Auth) -> Json<User> {
    Json(user)
}

/// Log in.
#[utoipa::path(
    post,
    path = "/session",
    tag = TAG_AUTH,
    request_body = Credentials,
    responses(
        (status = 200, body = User, description = "The session cookie is set"),
        (status = 401, body = Problem, description = "bad_credentials"),
    ),
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    WithRejection(Json(credentials), _): JsonBody<Credentials>,
) -> ApiResult<Response> {
    let (user, token) = state.service.login(credentials).await?;
    let cookie = auth::session_cookie(&token, state.service.session_lifetime());
    Ok(with_cookie(StatusCode::OK, &cookie, user))
}

/// Log out.
#[utoipa::path(delete, path = "/session", tag = TAG_AUTH, responses((status = 204)))]
pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(token) = auth::session_token(&headers) {
        state.service.logout(&token).await?;
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    insert(&mut response, header::SET_COOKIE, &auth::cleared_cookie());
    Ok(response)
}

fn with_cookie(status: StatusCode, cookie: &str, user: User) -> Response {
    let mut response = (status, Json(user)).into_response();
    insert(&mut response, header::SET_COOKIE, cookie);
    response
}
