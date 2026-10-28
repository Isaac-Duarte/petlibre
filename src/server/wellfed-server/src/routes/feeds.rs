//! Feeds and their cancels. A feed requires an `Idempotency-Key`, which
//! covers it from this request to its result: a retry under it is answered
//! from the journal and never feeds again.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum_extra::extract::WithRejection;
use uuid::Uuid;
use wellfed_api::{Command, FeedRequest, FeederId, Problem};

use super::{ApiResult, IdPath, JsonBody, TAG_FEEDERS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;
use crate::extract::{IdempotencyKey, OptionalIdempotencyKey};

/// Feed now. Answered once the feeder has reserved the feed: the journal
/// entry, with the feeder's number for it in `feed`. Its result arrives on
/// the stream and at `GET /commands/{key}`; a retry under the same key
/// returns the entry as it stands and never feeds again.
#[utoipa::path(
    post,
    path = "/feeders/{id}/feed",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), IdempotencyKey),
    request_body = FeedRequest,
    responses(
        (status = 202, body = Command),
        (status = 400, body = Problem, description = "invalid: a malformed body, or a missing or malformed Idempotency-Key"),
        (status = 403, body = Problem, description = "forbidden: a viewer"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "feeder_refused: nothing ran; final for this key, so a new feed needs a new key"),
        (status = 422, body = Problem, description = "key_reused: the key names another request; nothing was sent"),
        (status = 503, body = Problem, description = "feeder_offline: nothing was sent"),
        (status = 504, body = Problem, description = "outcome_unknown: never sent again; its result is read at GET /commands/{key}"),
    ),
)]
pub(crate) async fn feed(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::Feed>,
    WithRejection(Path(id), _): IdPath,
    IdempotencyKey(key): IdempotencyKey,
    WithRejection(Json(request), _): JsonBody<FeedRequest>,
) -> ApiResult<(StatusCode, Json<Command>)> {
    let command = state
        .service
        .feed(&caller, id, request.portions, key)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(command)))
}

/// Stop the feed with this number if it still runs. Its result is still
/// its `Fed` event, `cancelled`, with the portions it gave. A retry of a
/// cancel whose outcome was unknown may be refused `not_found`: the feed
/// has ended, perhaps by the first attempt, and its `Fed` event says how.
#[utoipa::path(
    post,
    path = "/feeders/{id}/feeds/{feed}/cancel",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), ("feed" = u32, Path), OptionalIdempotencyKey),
    responses(
        (status = 204),
        (status = 400, body = Problem, description = "invalid: a malformed Idempotency-Key"),
        (status = 403, body = Problem, description = "forbidden: a viewer"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "feeder_refused: not_found once the feed has ended"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn cancel(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::Feed>,
    WithRejection(Path((id, feed)), _): WithRejection<Path<(Uuid, u32)>, ApiError>,
    OptionalIdempotencyKey(key): OptionalIdempotencyKey,
) -> ApiResult<StatusCode> {
    state
        .service
        .cancel_feed(&caller, FeederId(id), feed, key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
