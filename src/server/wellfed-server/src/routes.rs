//! The handlers. Each parses, authorizes, calls one service method and
//! maps the result; the OpenAPI document comes from their annotations.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use wellfed_api::{
    AdoptFeeder, Credentials, ErrorCode, EventPage, EventQuery, Feeder, FeederDetail, FeederId,
    Generation, Health, LockRequest, Problem, Settings, SettingsPatch, SetupState, User,
};
use wellfed_service::IdempotencyKey;
use uuid::Uuid;

use crate::AppState;
use crate::auth::{self, Auth, Owner};
use crate::error::ApiError;

type ApiResult<T> = Result<T, ApiError>;
type JsonBody<T> = WithRejection<Json<T>, ApiError>;
type IdPath = WithRejection<Path<FeederId>, ApiError>;

pub(crate) const TAG_AUTH: &str = "auth";
pub(crate) const TAG_FEEDERS: &str = "feeders";

const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// Liveness and version, without authentication.
#[utoipa::path(get, path = "/health", responses((status = 200, body = Health)))]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

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

/// The adopted feeders, with the server's connection to each.
#[utoipa::path(
    get,
    path = "/feeders",
    tag = TAG_FEEDERS,
    responses((status = 200, body = Vec<Feeder>), (status = 401, body = Problem)),
)]
pub(crate) async fn feeders(
    State(state): State<AppState>,
    _: Auth,
) -> ApiResult<Json<Vec<Feeder>>> {
    Ok(Json(state.service.feeders().await?))
}

/// Adopt a feeder by redeeming an invite its owner made for the server.
#[utoipa::path(
    post,
    path = "/feeders",
    tag = TAG_FEEDERS,
    request_body = AdoptFeeder,
    responses(
        (status = 201, body = Feeder),
        (status = 400, body = Problem, description = "invalid: not an invite, or no address"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 409, body = Problem, description = "already_adopted, or feeder_refused"),
        (status = 410, body = Problem, description = "invite_expired"),
        (status = 502, body = Problem, description = "unreachable"),
    ),
)]
pub(crate) async fn adopt(
    State(state): State<AppState>,
    _: Owner,
    WithRejection(Json(request), _): JsonBody<AdoptFeeder>,
) -> ApiResult<(StatusCode, Json<Feeder>)> {
    let feeder = state.service.adopt(request).await?;
    Ok((StatusCode::CREATED, Json(feeder)))
}

/// One feeder and its status: live when it is online, otherwise the last
/// copy, marked.
#[utoipa::path(
    get,
    path = "/feeders/{id}",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses((status = 200, body = FeederDetail), (status = 404, body = Problem)),
)]
pub(crate) async fn feeder(
    State(state): State<AppState>,
    _: Auth,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Json<FeederDetail>> {
    Ok(Json(state.service.feeder(id).await?))
}

/// The feeder's settings, read from it. The `ETag` is the generation a
/// change sends back in `If-Match`.
#[utoipa::path(
    get,
    path = "/feeders/{id}/settings",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, body = Settings, headers(("ETag" = String))),
        (status = 503, body = Problem, description = "feeder_offline"),
    ),
)]
pub(crate) async fn settings(
    State(state): State<AppState>,
    _: Auth,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Response> {
    let settings = state.service.settings(id).await?;
    Ok(tagged(settings))
}

/// Change settings, if they are still at the generation in `If-Match`.
#[utoipa::path(
    patch,
    path = "/feeders/{id}/settings",
    tag = TAG_FEEDERS,
    params(
        ("id" = FeederId, Path),
        ("If-Match" = String, Header, description = "The generation the change is based on"),
        ("Idempotency-Key" = Option<String>, Header, description = "A UUID; a retry with the same key is answered once"),
    ),
    request_body = SettingsPatch,
    responses(
        (status = 200, body = Settings, headers(("ETag" = String))),
        (status = 412, body = Settings, description = "Stale: the current settings"),
        (status = 428, body = Problem, description = "precondition_required"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn put_settings(
    State(state): State<AppState>,
    Auth(user): Auth,
    WithRejection(Path(id), _): IdPath,
    headers: HeaderMap,
    WithRejection(Json(patch), _): JsonBody<SettingsPatch>,
) -> ApiResult<Response> {
    let base = if_match(&headers)?;
    let key = idempotency_key(&headers)?;
    let settings = state
        .service
        .put_settings(&user, id, base, patch, key)
        .await?;
    Ok(tagged(settings))
}

/// Lock or unlock the feeder's panel.
#[utoipa::path(
    post,
    path = "/feeders/{id}/lock",
    tag = TAG_FEEDERS,
    params(
        ("id" = FeederId, Path),
        ("Idempotency-Key" = Option<String>, Header, description = "A UUID; a retry with the same key is answered once"),
    ),
    request_body = LockRequest,
    responses(
        (status = 204),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn lock(
    State(state): State<AppState>,
    Auth(user): Auth,
    WithRejection(Path(id), _): IdPath,
    headers: HeaderMap,
    WithRejection(Json(request), _): JsonBody<LockRequest>,
) -> ApiResult<StatusCode> {
    let key = idempotency_key(&headers)?;
    state
        .service
        .set_lock(&user, id, request.locked, key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

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

/// A generation as an entity tag.
pub(crate) fn etag(generation: Generation) -> String {
    format!("\"{generation}\"")
}

fn tagged(settings: Settings) -> Response {
    let mut response = Json(settings).into_response();
    insert(&mut response, header::ETAG, &etag(settings.generation));
    response
}

/// The generation an edit is based on: `If-Match: "7"` (or bare `7`).
fn if_match(headers: &HeaderMap) -> ApiResult<Generation> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Err(ApiError::Code(
            ErrorCode::PreconditionRequired,
            Some("send If-Match with the generation the change is based on".into()),
        ));
    };
    value
        .to_str()
        .ok()
        .map(|text| text.trim().trim_matches('"'))
        .and_then(|text| text.parse().ok())
        .map(Generation)
        .ok_or_else(|| {
            ApiError::Code(
                ErrorCode::Invalid,
                Some("If-Match is a generation, such as \"7\"".into()),
            )
        })
}

/// A write's `Idempotency-Key` UUID, when the client sent one.
fn idempotency_key(headers: &HeaderMap) -> ApiResult<Option<IdempotencyKey>> {
    let Some(value) = headers.get(IDEMPOTENCY_KEY) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|text| Uuid::parse_str(text.trim()).ok())
        .map(|uuid| Some(*uuid.as_bytes()))
        .ok_or_else(|| ApiError::Code(ErrorCode::Invalid, Some("Idempotency-Key is a UUID".into())))
}

fn with_cookie(status: StatusCode, cookie: &str, user: User) -> Response {
    let mut response = (status, Json(user)).into_response();
    insert(&mut response, header::SET_COOKIE, cookie);
    response
}

fn insert(response: &mut Response, name: header::HeaderName, value: &str) {
    // Every value here is built from hex, digits and fixed text.
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}
