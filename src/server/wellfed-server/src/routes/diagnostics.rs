//! What the server reads from a feeder beside its history: a feed's trace,
//! the latest statistics, and the debug log followed live.

use std::convert::Infallible;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum_extra::extract::WithRejection;
use futures_util::{Stream, StreamExt, stream};
use wellfed_api::{FeedTrace, FeederId, FeederStats, LogEvent, Problem, TraceQuery};

use super::stream::session_ended;
use super::{ApiResult, IdPath, TAG_FEEDERS};
use crate::AppState;
use crate::auth::{self, Allowed, need};
use crate::error::ApiError;

/// A feed's trace: what its drive measured, poll by poll, as the server
/// fetched it after the feed's `Fed` event. `404` until it has been
/// fetched; a feeder that kept no trace of the feed leaves `missing`.
#[utoipa::path(
    get,
    path = "/feeders/{id}/feeds/{feed}/trace",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path), ("feed" = u32, Path), TraceQuery),
    responses((status = 200, body = FeedTrace), (status = 404, body = Problem)),
)]
pub(crate) async fn trace(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path((id, feed)), _): WithRejection<Path<(FeederId, u32)>, ApiError>,
    WithRejection(Query(query), _): WithRejection<Query<TraceQuery>, ApiError>,
) -> ApiResult<Json<FeedTrace>> {
    Ok(Json(state.service.feed_trace(id, feed, query).await?))
}

/// The latest read of each statistics section the feeder serves the
/// server, its lifetime counters among them.
#[utoipa::path(
    get,
    path = "/feeders/{id}/stats",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses((status = 200, body = FeederStats), (status = 404, body = Problem)),
)]
pub(crate) async fn stats(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Json<FeederStats>> {
    Ok(Json(state.service.stats(id).await?))
}

/// The feeder's debug log, live, as Server-Sent Events: from the oldest
/// line it keeps, or from now when another owner already follows it. The
/// server tails the log only while someone follows, and stores none of
/// it. The stream ends when the feeder's tail does; follow again. The
/// server's key must be an owner's on the feeder, or the feeder refuses
/// it `forbidden`.
#[utoipa::path(
    get,
    path = "/feeders/{id}/log",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, content_type = "text/event-stream", body = LogEvent),
        (status = 404, body = Problem),
        (status = 409, body = Problem),
        (status = 503, body = Problem),
    ),
)]
pub(crate) async fn log(
    State(state): State<AppState>,
    _: Allowed<need::FeederAdmin>,
    headers: HeaderMap,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let follow = state.service.follow_log(id).await?;

    let batches = stream::unfold(follow, |mut follow| async move {
        let event = follow.next().await?;
        Some((Ok(encode(&event)), follow))
    });
    let shutdown = state.shutdown.clone();
    let session = session_ended(state, auth::credential(&headers));
    let events = batches.take_until(async move {
        tokio::select! {
            () = shutdown.cancelled() => {}
            () = session => {}
        }
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

fn encode(event: &LogEvent) -> Event {
    // These types serialize without fail; a comment keeps the stream going
    // if one ever did not.
    Event::default()
        .json_data(event)
        .unwrap_or_else(|_| Event::default().comment("unencodable log batch"))
}
