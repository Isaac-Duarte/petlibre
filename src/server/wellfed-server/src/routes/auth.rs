//! First-owner setup and the session: logging in and out.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use wellfed_api::{Credentials, PasswordChange, Problem, Setup, SetupState, User};

use super::{ApiResult, JsonBody, TAG_AUTH, insert};
use crate::AppState;
use crate::auth::{self, Allowed, Auth, need};
use crate::proxy::Remote;

/// Whether a fresh server is waiting for its first owner, and the host's
/// zone, which the setup page offers as the household's.
#[utoipa::path(get, path = "/setup", tag = TAG_AUTH, responses((status = 200, body = SetupState)))]
pub(crate) async fn setup_state(State(state): State<AppState>) -> ApiResult<Json<SetupState>> {
    Ok(Json(state.service.setup_state().await?))
}

/// Create the first owner and the household in its zone, and log the owner
/// in. Closed once any account exists.
#[utoipa::path(
    post,
    path = "/setup",
    tag = TAG_AUTH,
    request_body = Setup,
    responses(
        (status = 201, body = User, description = "Created; the session cookie is set"),
        (status = 400, body = Problem, description = "invalid: a name, a password under 10 characters, or an unknown zone"),
        (status = 409, body = Problem, description = "setup_closed"),
        (status = 422, body = Problem, description = "zone_unsupported: no POSIX rule describes the zone"),
    ),
)]
pub(crate) async fn setup(
    State(state): State<AppState>,
    remote: Remote,
    WithRejection(Json(setup), _): JsonBody<Setup>,
) -> ApiResult<Response> {
    let (user, token) = state.service.setup(setup).await?;
    let cookie = auth::session_cookie(&token, remote.scheme);
    Ok(with_cookie(StatusCode::CREATED, &cookie, user))
}

/// The logged-in user.
#[utoipa::path(
    get,
    path = "/session",
    tag = TAG_AUTH,
    responses(
        (status = 200, body = User),
        (status = 401, body = Problem),
        (status = 403, body = Problem, description = "account_disabled"),
    ),
)]
pub(crate) async fn session(Auth(caller): Auth) -> Json<User> {
    Json(caller.user)
}

/// Log in. The session lasts 30 days from its last use. Failed logins are
/// limited to 5 a minute per name and 20 a minute per address; no account
/// is ever locked.
#[utoipa::path(
    post,
    path = "/session",
    tag = TAG_AUTH,
    request_body = Credentials,
    responses(
        (status = 200, body = User, description = "The session cookie is set, `Secure` behind a trusted HTTPS proxy"),
        (status = 401, body = Problem, description = "bad_credentials"),
        (status = 403, body = Problem, description = "account_disabled: past its expiry"),
        (status = 429, body = Problem, description = "login_limited: too many failed logins for the name or from the address",
            headers(("Retry-After" = u64, description = "Seconds until the next login is taken"))),
    ),
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    remote: Remote,
    WithRejection(Json(credentials), _): JsonBody<Credentials>,
) -> ApiResult<Response> {
    let (user, token) = state.service.login(credentials, remote.address).await?;
    let cookie = auth::session_cookie(&token, remote.scheme);
    Ok(with_cookie(StatusCode::OK, &cookie, user))
}

/// Change one's own password, given the current one. Every other session
/// of the account ends; this one stays, and so do the account's tokens.
/// Only from a login, never a token.
#[utoipa::path(
    patch,
    path = "/session",
    tag = TAG_AUTH,
    request_body = PasswordChange,
    responses(
        (status = 204),
        (status = 400, body = Problem, description = "invalid: a password under 10 characters"),
        (status = 401, body = Problem, description = "bad_credentials: not the current password"),
        (status = 403, body = Problem, description = "session_required: not from a token"),
    ),
)]
pub(crate) async fn change_password(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnAccount>,
    headers: HeaderMap,
    WithRejection(Json(change), _): JsonBody<PasswordChange>,
) -> ApiResult<StatusCode> {
    let session = auth::session(&headers);
    state
        .service
        .change_password(&caller, session.as_ref(), change)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Log out.
#[utoipa::path(delete, path = "/session", tag = TAG_AUTH, responses((status = 204)))]
pub(crate) async fn logout(
    State(state): State<AppState>,
    remote: Remote,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(token) = auth::session(&headers) {
        state.service.logout(&token).await?;
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    let cleared = auth::cleared_cookie(remote.scheme);
    insert(&mut response, header::SET_COOKIE, &cleared);
    Ok(response)
}

fn with_cookie(status: StatusCode, cookie: &str, user: User) -> Response {
    let mut response = (status, Json(user)).into_response();
    insert(&mut response, header::SET_COOKIE, cookie);
    response
}
