//! Following a feeder's debug log: a session of its own, opened for the
//! first follower and closed once the last has gone.
//!
//! The feeder has no op that stops a tail: once a session asks for the log
//! (`TailLog`), the feeder pushes every new line on it until the session
//! ends. So the tail never runs on the feeder task's session, where it
//! would go on for as long as that session lasts and share its frames
//! with the feeds and the history. It runs on a second session with the
//! same key, which ends when nobody follows, when the feeder's task stops,
//! or when the feeder drops it; a follower then sees its log end and
//! follows again. Nothing of the log is stored, and a follower who joins
//! a running tail gets the lines from then on.
//!
//! The feeder serves four sessions at once, so a followed log takes one
//! from the CLI, the phones and the server's own. The server's own goes
//! first: when the feeder task finds every session taken as it
//! reconnects, it ends the tail and tries again.
//!
//! The tail's task owns the only strong sender of its lines; the feeder's
//! handle keeps a weak one. So when the task ends, for whatever reason,
//! every follower's receiver closes, and a follower who comes after finds
//! no tail to join and starts another.

use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use wellfed_api::{Connection as Link, FeederId};
use wellfed_connect::Connection;
use wellfed_model::{Answer, LogLines, Op, Pushed};
use wellfed_session::noise::Keypair;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, info_span};

use crate::{LogError, Mirror, Shared, connect};

/// How many batches a slow follower may fall behind before it is told it
/// lagged.
const BATCHES: usize = 64;

/// How often a running tail checks that someone still follows it, when
/// the feeder logs nothing meanwhile.
const FOLLOWERS: Duration = Duration::from_secs(1);

/// A feeder's running tail as its handle keeps it: the weak sender of its
/// lines, under the lock a first follower holds while the tail's session
/// opens, so one feeder's followers take turns and no other feeder's wait.
pub(crate) type Slot = Arc<tokio::sync::Mutex<Option<broadcast::WeakSender<LogLines>>>>;

/// What ends a feeder's running tail early: its feeder task cancels it to
/// take back the session.
pub(crate) type Stop = Arc<std::sync::Mutex<Option<CancellationToken>>>;

/// The running tail's stop, if any. Insertions are whole, so a poisoned
/// lock still holds a consistent value.
pub(crate) fn stop(stop: &Stop) -> MutexGuard<'_, Option<CancellationToken>> {
    stop.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a tail's session goes, and what ends it.
struct Target {
    id: FeederId,
    identity: [u8; 32],
    keys: Keypair,
    address: Option<String>,
    cancel: CancellationToken,
}

impl Mirror {
    /// Follow the feeder's debug log: its record lines from the oldest it
    /// keeps (or, joining a running tail, from now), as the feeder sends
    /// them. The receiver closes when the tail ends. Only an owner's key
    /// may read the log: on another, the feeder's `Forbidden` comes back.
    pub async fn follow_log(
        &self,
        id: FeederId,
    ) -> Result<broadcast::Receiver<LogLines>, LogError> {
        let (slot, stops, target) = {
            let handles = self.handles();
            let handle = handles.get(&id).ok_or(LogError::NotRunning)?;
            if handle.link() != Link::Online {
                return Err(LogError::Offline);
            }
            let target = Target {
                id,
                identity: handle.identity,
                keys: handle.keys.clone(),
                address: handle.address.borrow().clone(),
                cancel: handle.cancel.child_token(),
            };
            (
                Arc::clone(&handle.log),
                Arc::clone(&handle.log_stop),
                target,
            )
        };

        let mut slot = slot.lock().await;
        if let Some(lines) = slot.as_ref().and_then(broadcast::WeakSender::upgrade) {
            return Ok(lines.subscribe());
        }

        let (lines, follower) = broadcast::channel(BATCHES);
        let weak = lines.downgrade();
        let (started, start) = oneshot::channel();
        stop(&stops).replace(target.cancel.clone());
        let span = info_span!(parent: None, "feeder", id = %id);
        self.tasks
            .spawn(tail(Arc::clone(&self.shared), target, lines, started).instrument(span));
        start.await.map_err(|_| LogError::Unanswered)??;

        *slot = Some(weak);
        Ok(follower)
    }
}

/// Open the tail's session and ask for the log, telling `started` how
/// that went; then forward each batch until nobody follows, the feeder's
/// task stops or takes the session back, or the session ends. `lines` is
/// the only strong sender, so every follower's receiver closes as this
/// returns.
async fn tail(
    shared: Arc<Shared>,
    target: Target,
    lines: broadcast::Sender<LogLines>,
    started: oneshot::Sender<Result<(), LogError>>,
) {
    let cancel = target.cancel.clone();
    let opened = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(LogError::Offline),
        opened = open(&shared, &target) => opened,
    };
    let mut connection = match opened {
        Ok(connection) => connection,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    if started.send(Ok(())).is_err() {
        return;
    }
    info!("following the debug log");

    let mut check = tokio::time::interval(FOLLOWERS);
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            _ = check.tick() => {
                if lines.receiver_count() == 0 {
                    break;
                }
            }
            pushed = connection.next(FOLLOWERS) => match pushed {
                Ok(Pushed::Log(batch)) => {
                    if lines.send(batch).is_err() {
                        break;
                    }
                }
                // The tail's session never subscribes, so nothing else is
                // pushed on it.
                Ok(_) | Err(wellfed_connect::Error::Timeout) => {}
                Err(error) => {
                    debug!("the debug log's session ended: {error}");
                    break;
                }
            },
        }
    }
    info!("no longer following the debug log");
}

/// The tail's session, with the log asked for.
async fn open(shared: &Shared, target: &Target) -> Result<Connection, LogError> {
    let (mut connection, _) = connect::open(
        shared,
        target.id,
        target.identity,
        &target.keys,
        target.address.as_deref(),
    )
    .await?;
    if !connection.capabilities().log {
        return Err(LogError::Refused(wellfed_model::Refusal::Unavailable));
    }

    let request = shared.random().map_err(crate::ConnectError::from)?;
    match connection.ask(request, &Op::TailLog { after: None }).await {
        Ok(Answer::Done { .. }) => Ok(connection),
        Ok(Answer::Refused(refusal)) => Err(LogError::Refused(refusal)),
        Ok(_) => Err(LogError::Unexpected),
        Err(_) => Err(LogError::Unanswered),
    }
}
