//! Adopted feeders: the list, adoption, one feeder's status, its settings
//! and its panel lock.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use wellfed_api::{
    AdoptFeeder, Feeder, FeederDetail, FeederId, Generation, LockRequest, Problem, Settings,
    SettingsPatch,
};

use super::{ApiResult, IdPath, JsonBody, TAG_FEEDERS, insert};
use crate::AppState;
use crate::auth::{Auth, Owner};
use crate::extract::{IfMatch, OptionalIdempotencyKey};

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
        (status = 409, body = Problem, description = "feeder_refused"),
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
    params(("id" = FeederId, Path), IfMatch, OptionalIdempotencyKey),
    request_body = SettingsPatch,
    responses(
        (status = 200, body = Settings, headers(("ETag" = String))),
        (status = 412, body = Settings, description = "Stale: the current settings"),
        (status = 428, body = Problem, description = "precondition_required"),
        (status = 409, body = Problem, description = "feeder_refused: final for this key; a later attempt needs a new key"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn put_settings(
    State(state): State<AppState>,
    Auth(user): Auth,
    WithRejection(Path(id), _): IdPath,
    IfMatch(base): IfMatch,
    OptionalIdempotencyKey(key): OptionalIdempotencyKey,
    WithRejection(Json(patch), _): JsonBody<SettingsPatch>,
) -> ApiResult<Response> {
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
    params(("id" = FeederId, Path), OptionalIdempotencyKey),
    request_body = LockRequest,
    responses(
        (status = 204),
        (status = 409, body = Problem, description = "feeder_refused: final for this key; a later attempt needs a new key"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn lock(
    State(state): State<AppState>,
    Auth(user): Auth,
    WithRejection(Path(id), _): IdPath,
    OptionalIdempotencyKey(key): OptionalIdempotencyKey,
    WithRejection(Json(request), _): JsonBody<LockRequest>,
) -> ApiResult<StatusCode> {
    state
        .service
        .set_lock(&user, id, request.locked, key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
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
