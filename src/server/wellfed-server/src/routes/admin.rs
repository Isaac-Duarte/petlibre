//! What an owner administers on a feeder: its Wi-Fi, a restart and
//! removing it; and a tune, which a member may play too.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum_extra::extract::WithRejection;
use wellfed_api::{FeederId, PlayRequest, Problem, Removal, WifiChange};

use super::{ApiResult, IdPath, JsonBody, TAG_FEEDERS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::extract::{IfMatch, OptionalIdempotencyKey};

/// Store Wi-Fi credentials for the feeder's next join, or clear them, if
/// its Wi-Fi store is still at the generation in `If-Match` (the status's
/// `wifi_generation`). The passphrase is never stored or echoed. A change
/// whose answer was lost and is retried after the feeder restarted can be
/// answered `stale` though it ran: the status's `wifi_generation` says.
#[utoipa::path(
    put,
    path = "/feeders/{id}/wifi",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), IfMatch, OptionalIdempotencyKey),
    request_body = WifiChange,
    responses(
        (status = 204),
        (status = 400, body = Problem, description = "invalid: an SSID or passphrase out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "feeder_archived, or feeder_refused (stale: read the status again; forbidden: the server's key is not an owner's)"),
        (status = 428, body = Problem, description = "precondition_required"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn put_wifi(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::FeederAdmin>,
    WithRejection(Path(id), _): IdPath,
    IfMatch(base): IfMatch,
    OptionalIdempotencyKey(key): OptionalIdempotencyKey,
    WithRejection(Json(change), _): JsonBody<WifiChange>,
) -> ApiResult<StatusCode> {
    state
        .service
        .put_wifi(&caller, id, base, change, key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Restart the feeder; refused `busy` while a feed is queued or runs. One
/// whose outcome is unknown is never resent: the boot count says whether
/// it ran.
#[utoipa::path(
    post,
    path = "/feeders/{id}/reboot",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), OptionalIdempotencyKey),
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "feeder_archived, or feeder_refused"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: never resent"),
    ),
)]
pub(crate) async fn reboot(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::FeederAdmin>,
    WithRejection(Path(id), _): IdPath,
    OptionalIdempotencyKey(key): OptionalIdempotencyKey,
) -> ApiResult<StatusCode> {
    state.service.reboot(&caller, id, key).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Play a tune now; refused `muted` while the sound setting is off. The
/// firmware steps its speaker only in setup mode for now, so it refuses
/// `unavailable` in the loop.
#[utoipa::path(
    post,
    path = "/feeders/{id}/play",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), OptionalIdempotencyKey),
    request_body = PlayRequest,
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: a viewer"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "feeder_archived, or feeder_refused"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: retry with the same key"),
    ),
)]
pub(crate) async fn play(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::Control>,
    WithRejection(Path(id), _): IdPath,
    OptionalIdempotencyKey(key): OptionalIdempotencyKey,
    WithRejection(Json(request), _): JsonBody<PlayRequest>,
) -> ApiResult<StatusCode> {
    state.service.play(&caller, id, request, key).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove the feeder from the server: revoke the phone keys still owed a
/// revoke on it and the server's own key, stop mirroring it and archive
/// it, whether or not the revokes landed, which the answer says. Its history, samples and journal stay readable;
/// adopting it again brings it back with them.
#[utoipa::path(
    delete,
    path = "/feeders/{id}",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, body = Removal),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "feeder_archived: already removed"),
    ),
)]
pub(crate) async fn remove(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::Adopt>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Json<Removal>> {
    Ok(Json(state.service.remove_feeder(&caller, id).await?))
}
