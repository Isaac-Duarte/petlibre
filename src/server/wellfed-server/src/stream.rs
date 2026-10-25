//! `GET /api/v1/stream`: the hub's updates as Server-Sent Events.
//!
//! Each event's `id` is its [`StreamId`]; a browser's `EventSource` sends
//! the last one back as `Last-Event-ID` when it reconnects, and gets what
//! it missed or a `resync`. A subscriber that falls behind the hub also
//! gets a `resync` rather than a silent hole. Every stream ends when the
//! server shuts down, so shutdown never waits on an open browser tab, and
//! within [`SESSION_CHECK`] of its session ending (logout or expiry).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum_extra::extract::WithRejection;
use futures_util::{Stream, StreamExt, stream};
use wellfed_api::{ErrorCode, MetricKind, Problem, StreamEvent, StreamQuery};
use wellfed_service::{Resume, SessionToken, Stamped, StreamId};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::AppState;
use crate::auth::{self, Auth};
use crate::error::ApiError;
use crate::routes::TAG_FEEDERS;

/// How often an open stream checks that its session still holds.
const SESSION_CHECK: Duration = Duration::from_secs(60);

/// Live updates for every feeder: events, gaps, connection changes, and
/// the sample metrics asked for.
#[utoipa::path(
    get,
    path = "/stream",
    tag = TAG_FEEDERS,
    params(
        StreamQuery,
        ("Last-Event-ID" = Option<String>, Header, description = "Resume after this event"),
    ),
    responses(
        (status = 200, content_type = "text/event-stream", body = StreamEvent),
        (status = 400, body = Problem),
        (status = 401, body = Problem),
    ),
)]
pub(crate) async fn stream(
    State(state): State<AppState>,
    _: Auth,
    headers: HeaderMap,
    WithRejection(Query(query), _): WithRejection<Query<StreamQuery>, ApiError>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let metrics = metrics(query.samples.as_deref())?;
    // An id this server could not have sent resumes nowhere: resync.
    let last = headers
        .get("last-event-id")
        .map(|id| id.to_str().ok().and_then(|id| id.parse().ok()));
    let last = match last {
        Some(Some(id)) => Some(id),
        Some(None) => Some(StreamId { epoch: 0, seq: 0 }),
        None => None,
    };
    let subscription = state.service.hub().subscribe(last);

    let wanted = move |event: &StreamEvent| match event {
        StreamEvent::Sample { sample, .. } => metrics.contains(&sample.value.metric()),
        _ => true,
    };
    let first: Vec<_> = match subscription.resume {
        Resume::Replay(missed) => missed
            .into_iter()
            .filter(|stamped| wanted(&stamped.event))
            .map(|stamped| Ok(sse(&stamped)))
            .collect(),
        Resume::Resync => vec![Ok(resync())],
    };
    let live = BroadcastStream::new(subscription.live).filter_map(move |update| {
        let event = match update {
            Ok(stamped) => wanted(&stamped.event).then(|| sse(&stamped)),
            Err(BroadcastStreamRecvError::Lagged(_)) => Some(resync()),
        };
        std::future::ready(event.map(Ok))
    });

    let shutdown = state.shutdown.clone();
    let session = session_ended(state, auth::session_token(&headers));
    let events = stream::iter(first).chain(live).take_until(async move {
        tokio::select! {
            () = shutdown.cancelled() => {}
            () = session => {}
        }
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

/// Resolves once the stream's session no longer authenticates. A store
/// failure ends the stream too; the client reconnects and is checked again.
async fn session_ended(state: AppState, token: Option<SessionToken>) {
    let Some(token) = token else {
        return;
    };
    loop {
        tokio::time::sleep(SESSION_CHECK).await;
        if !matches!(state.service.authenticate(&token).await, Ok(Some(_))) {
            return;
        }
    }
}

/// The metrics a `samples` list names.
fn metrics(list: Option<&str>) -> Result<Vec<MetricKind>, ApiError> {
    let Some(list) = list else {
        return Ok(Vec::new());
    };
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            MetricKind::from_code(name).ok_or_else(|| {
                ApiError::Code(ErrorCode::Invalid, Some(format!("no metric {name:?}")))
            })
        })
        .collect()
}

fn sse(stamped: &Arc<Stamped>) -> Event {
    encode(&stamped.event).id(stamped.id.to_string())
}

/// The client must refetch. Its empty `id` clears the browser's last event
/// ID, so a reconnect before the next update starts afresh instead of
/// resending the id that could not resume.
fn resync() -> Event {
    encode(&StreamEvent::Resync).id("")
}

fn encode(event: &StreamEvent) -> Event {
    // These types serialize without fail; a comment keeps the stream going
    // if one ever did not.
    Event::default()
        .json_data(event)
        .unwrap_or_else(|_| Event::default().comment("unencodable update"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_list_names_known_metrics_only() {
        assert_eq!(metrics(None).unwrap(), []);
        assert_eq!(
            metrics(Some("power, rssi,")).unwrap(),
            [MetricKind::Power, MetricKind::Rssi]
        );
        assert!(metrics(Some("power,temperature")).is_err());
    }
}
