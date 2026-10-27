//! `GET /api/v1/stream`: the hub's updates as Server-Sent Events.
//!
//! Each event's `id` is its [`StreamId`]; a browser's `EventSource` sends
//! the last one back as `Last-Event-ID` when it reconnects, and gets what
//! it missed or a `resync`. A subscriber that falls behind the hub also
//! gets a `resync` rather than a silent hole. Every stream ends when the
//! server shuts down, so shutdown never waits on an open browser tab, and
//! within [`SESSION_CHECK`] of its session or token ending (logout,
//! revocation or expiry).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum_extra::extract::WithRejection;
use futures_util::{Stream, StreamExt, stream};
use wellfed_api::{ErrorCode, MetricKind, Problem, StreamEvent, StreamQuery, User};
use wellfed_service::{Permission, Resume, Stamped, StreamId, can};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::TAG_FEEDERS;
use crate::AppState;
use crate::auth::{self, Allowed, Credential, need};
use crate::error::ApiError;

/// How often an open stream checks that its session or token still holds.
const SESSION_CHECK: Duration = Duration::from_secs(60);

/// Live updates for every feeder: events, gaps, connection changes, the
/// sample metrics asked for, and changes to the caller's journal entries
/// (everyone's for an owner), a feed's result among them.
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
    Allowed(caller, _): Allowed<need::Read>,
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

    let wanted = move |event: &StreamEvent| shown(&caller.user, &metrics, event);
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
    let session = session_ended(state, auth::credential(&headers));
    let events = stream::iter(first).chain(live).take_until(async move {
        tokio::select! {
            () = shutdown.cancelled() => {}
            () = session => {}
        }
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

/// Resolves once the stream's credential no longer authenticates. A store
/// failure ends the stream too; the client reconnects and is checked again.
pub(crate) async fn session_ended(state: AppState, credential: Option<Credential>) {
    let Some(credential) = credential else {
        return;
    };
    loop {
        tokio::time::sleep(SESSION_CHECK).await;
        if !matches!(credential.caller(&state).await, Ok(Some(_))) {
            return;
        }
    }
}

/// Whether `user`'s stream carries `event`: a sample only of the metrics
/// asked for, and a journal entry as `GET /commands` shows it, a member's
/// own only.
fn shown(user: &User, metrics: &[MetricKind], event: &StreamEvent) -> bool {
    match event {
        StreamEvent::Sample { sample, .. } => metrics.contains(&sample.value.metric()),
        StreamEvent::Command { command } => {
            can(user.role, Permission::AllHistory) || command.user == user.id
        }
        StreamEvent::Event { .. }
        | StreamEvent::Gap { .. }
        | StreamEvent::Connection { .. }
        | StreamEvent::Install { .. }
        | StreamEvent::Resync => true,
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
    use jiff::Timestamp;
    use wellfed_api::{Command, CommandKind, CommandState, FeederId, Role, UserId};
    use uuid::Uuid;

    use super::*;

    fn user(n: u128, role: Role) -> User {
        User {
            id: UserId(Uuid::from_u128(n)),
            name: format!("user {n}"),
            role,
            expires: None,
        }
    }

    #[test]
    fn a_member_is_streamed_their_own_journal_entries_and_an_owner_everyones() {
        let entry = |by: &User| StreamEvent::Command {
            command: Command {
                key: Uuid::from_u128(9),
                user: by.id,
                token: None,
                feeder: FeederId(Uuid::from_u128(3)),
                kind: CommandKind::Feed,
                state: CommandState::Resolved,
                outcome: None,
                feed: Some(1),
                boot_count: None,
                sent: Timestamp::UNIX_EPOCH,
                updated: Timestamp::UNIX_EPOCH,
            },
        };
        let viewer = user(4, Role::Viewer);
        assert!(!shown(&viewer, &[], &entry(&user(2, Role::Member))));
        let (owner, member, other) = (
            user(1, Role::Owner),
            user(2, Role::Member),
            user(3, Role::Member),
        );

        assert!(shown(&owner, &[], &entry(&member)));
        assert!(shown(&member, &[], &entry(&member)));
        assert!(!shown(&other, &[], &entry(&member)));
        assert!(!shown(&member, &[], &entry(&owner)));
        assert!(shown(&member, &[], &StreamEvent::Resync));
    }

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
