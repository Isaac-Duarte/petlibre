//! The releases the server keeps for installs: the list, an owner's
//! upload, and a check of the project's GitHub releases. Owners only.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum_extra::extract::WithRejection;
use wellfed_api::{Problem, Release, ReleaseCheck, ReleasePage, ReleaseQuery};
use wellfed_service::ReleaseAdded;

use super::{ApiResult, TAG_RELEASES};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// A release file's bytes, as the upload's body documents them.
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
#[expect(
    dead_code,
    reason = "only its schema is used: the body arrives as bytes"
)]
pub(crate) struct ReleaseFile(Vec<u8>);

/// The verified releases the server holds, newest version first.
#[utoipa::path(
    get,
    path = "/releases",
    tag = TAG_RELEASES,
    params(ReleaseQuery),
    responses(
        (status = 200, body = ReleasePage),
        (status = 400, body = Problem, description = "invalid: a bad `after` or `limit`"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn releases(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
    WithRejection(Query(query), _): WithRejection<Query<ReleaseQuery>, ApiError>,
) -> ApiResult<Json<ReleasePage>> {
    Ok(Json(state.service.releases(&query).await?))
}

/// Upload a release file (a signed manifest, then the image it names),
/// for an offline home or a self-built image. It is verified as a fetched
/// one is, against the project's keys and the owner's.
#[utoipa::path(
    post,
    path = "/releases",
    tag = TAG_RELEASES,
    request_body(content = ReleaseFile, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = Release, description = "verified and kept"),
        (status = 200, body = Release, description = "the server held it already"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 413, body = Problem, description = "too_large: longer than any release"),
        (status = 422, body = Problem, description = "release_refused: `release` says why"),
    ),
)]
pub(crate) async fn upload_release(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
    WithRejection(file, _): WithRejection<Bytes, ApiError>,
) -> ApiResult<(StatusCode, Json<Release>)> {
    let (release, added) = state.service.upload_release(file.into()).await?;
    let status = match added {
        ReleaseAdded::New => StatusCode::CREATED,
        ReleaseAdded::Known => StatusCode::OK,
    };
    Ok((status, Json(release)))
}

/// Check the project's GitHub releases now, keeping each new one that
/// verifies. The server also checks daily while fetching is on.
#[utoipa::path(
    post,
    path = "/releases/check",
    tag = TAG_RELEASES,
    responses(
        (status = 200, body = ReleaseCheck),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 409, body = Problem, description = "fetch_disabled: `releases.fetch` is off"),
        (status = 502, body = Problem, description = "releases_unreachable"),
    ),
)]
pub(crate) async fn check_releases(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
) -> ApiResult<Json<ReleaseCheck>> {
    Ok(Json(state.service.check_releases().await?))
}
