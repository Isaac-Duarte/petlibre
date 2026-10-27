//! One feeder's task: connect, serve the session until it ends, wait with
//! jittered backoff, and connect again, until it is stopped.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{Connection as Link, FeederId};
use wellfed_connect::Connection;
use wellfed_session::noise::Keypair;
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, warn};

use crate::command::Command;
use crate::session::End;
use crate::{ConnectError, Feeder, MirrorEvent, Shared, Unreachable, connect};

/// Commands waiting for the task; more wait in `send`.
const COMMANDS: usize = 16;

/// The mirror's side of a task.
pub(crate) struct Handle {
    pub(crate) commands: mpsc::Sender<Command>,
    pub(crate) link: watch::Receiver<Link>,
    /// The long-operation turn: one permit.
    pub(crate) long: Arc<Semaphore>,
    updates: broadcast::Sender<MirrorEvent>,
    cancel: CancellationToken,
}

impl Handle {
    pub(crate) fn link(&self) -> Link {
        *self.link.borrow()
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<MirrorEvent> {
        self.updates.subscribe()
    }

    pub(crate) fn stop(self) {
        self.cancel.cancel();
    }
}

pub(crate) fn spawn(
    shared: Arc<Shared>,
    feeder: Feeder,
    updates: broadcast::Sender<MirrorEvent>,
    cancel: CancellationToken,
    tasks: &TaskTracker,
) -> Handle {
    let (commands, receiver) = mpsc::channel(COMMANDS);
    let (link, watching) = watch::channel(Link::Offline);
    let task = Task {
        shared,
        id: feeder.id,
        identity: feeder.identity,
        keys: Keypair::from_secret(feeder.secret),
        address: feeder.address,
        commands: receiver,
        link,
        updates: updates.clone(),
        cancel: cancel.clone(),
    };
    tasks.spawn(task.run());
    Handle {
        commands,
        link: watching,
        long: Arc::new(Semaphore::new(1)),
        updates,
        cancel,
    }
}

pub(crate) struct Task {
    pub(crate) shared: Arc<Shared>,
    pub(crate) id: FeederId,
    identity: [u8; 32],
    keys: Keypair,
    address: Option<String>,
    pub(crate) commands: mpsc::Receiver<Command>,
    link: watch::Sender<Link>,
    pub(crate) updates: broadcast::Sender<MirrorEvent>,
    pub(crate) cancel: CancellationToken,
}

impl Task {
    async fn run(mut self) {
        let mut failures = 0u32;
        loop {
            self.set(Link::Connecting);
            let Some(opened) = self.open().await else {
                return;
            };

            let link = match opened {
                Ok((connection, address)) => {
                    info!(feeder = %self.id, %address, build = connection.build(), "connected");
                    self.address = Some(address.to_string());
                    let opened_at = Instant::now();
                    let end = self.serve(connection).await;
                    // Only a session that lasted resets the backoff, so a
                    // feeder that accepts and drops at once, or a store
                    // that keeps failing, is not retried every second.
                    if opened_at.elapsed() >= self.shared.config.timing.retry_max {
                        failures = 0;
                    }
                    match end {
                        End::Cancelled => return,
                        End::Lost(error) => info!(feeder = %self.id, "session ended: {error}"),
                        End::Failed(error) => warn!(feeder = %self.id, "session dropped: {error}"),
                    }
                    Link::Offline
                }
                Err(ConnectError::Unreachable(Unreachable(
                    wellfed_connect::Error::UnknownKey,
                ))) => {
                    warn!(feeder = %self.id, "the feeder no longer knows this server's key");
                    Link::Refused
                }
                Err(error) => {
                    debug!(feeder = %self.id, "connect failed: {error}");
                    Link::Offline
                }
            };
            self.set(link);

            let wait = match link {
                Link::Refused => self.shared.config.timing.retry_max,
                _ => self.backoff(failures),
            };
            failures = failures.saturating_add(1);
            if !self.idle(wait).await {
                return;
            }
        }
    }

    /// Open a session, turning commands away meanwhile; `None` when the
    /// task should stop.
    async fn open(&mut self) -> Option<Result<(Connection, SocketAddr), ConnectError>> {
        let open = connect::open(
            &self.shared,
            self.id,
            self.identity,
            &self.keys,
            self.address.as_deref(),
        );
        tokio::pin!(open);
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return None,
                command = self.commands.recv() => {
                    command?.offline();
                }
                opened = &mut open => return Some(opened),
            }
        }
    }

    /// Record the session's state and, when it changed, tell subscribers.
    pub(crate) fn set(&self, link: Link) {
        let changed = self.link.send_if_modified(|current| {
            let changed = *current != link;
            *current = link;
            changed
        });
        if changed {
            // No subscriber is fine.
            let _ = self.updates.send(MirrorEvent::Link(link));
        }
    }

    /// Wait before reconnecting, turning commands away meanwhile; false
    /// when the task should stop.
    async fn idle(&mut self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return false,
                command = self.commands.recv() => {
                    let Some(command) = command else { return false };
                    command.offline();
                }
                () = tokio::time::sleep_until(deadline) => return true,
            }
        }
    }

    /// The delay after `failures` failed connects: doubling from the
    /// minimum up to the maximum, then jittered to between half and all of
    /// it, so feeders back after a shared outage do not reconnect in step.
    fn backoff(&self, failures: u32) -> Duration {
        let timing = self.shared.config.timing;
        let full = timing
            .retry_min
            .saturating_mul(1 << failures.min(16))
            .min(timing.retry_max);
        let [jitter] = self.shared.random::<1>().unwrap_or([u8::MAX]);
        full / 2 + (full / 2).mul_f32(f32::from(jitter) / f32::from(u8::MAX))
    }
}
