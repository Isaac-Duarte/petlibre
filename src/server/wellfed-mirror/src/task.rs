//! One feeder's task: connect, serve the session until it ends, wait with
//! jittered backoff, and connect again, until it is stopped.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{Connection as Link, FeederId};
use wellfed_connect::Connection;
use wellfed_model::Capabilities;
use wellfed_session::noise::Keypair;
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{Instrument, debug, info, info_span, warn};

use crate::collect::Collector;
use crate::command::Command;
use crate::ingest::Batch;
use crate::session::End;
use crate::{
    ConnectError, Diagnostics, Feeder, MirrorEvent, Shared, Unreachable, Zone, connect, log,
};

/// Commands waiting for the task; more wait in `send`.
const COMMANDS: usize = 16;
/// A long operation sends one step at a time, so one waits at most.
const STEPS: usize = 1;

/// The mirror's side of a task.
pub(crate) struct Handle {
    /// The short lane.
    pub(crate) commands: mpsc::Sender<Command>,
    /// The long lane, for the [`crate::Operation`] holding the turn.
    pub(crate) long: LongLane,
    pub(crate) link: watch::Receiver<Link>,
    /// The open session; `None` between sessions.
    opened: watch::Receiver<Option<Opened>>,
    /// Who the feeder is and the server's key on it, for a session of the
    /// debug log's own.
    pub(crate) identity: [u8; 32],
    pub(crate) keys: Keypair,
    /// Where the task last reached the feeder.
    pub(crate) address: watch::Receiver<Option<String>>,
    /// The debug log's running tail, and what ends it.
    pub(crate) log: log::Slot,
    pub(crate) log_stop: log::Stop,
    updates: broadcast::Sender<MirrorEvent>,
    pub(crate) cancel: CancellationToken,
    task: JoinHandle<()>,
    collector: Option<JoinHandle<()>>,
}

/// The long lane and its turn: whoever holds the turn's one permit sends
/// steps.
#[derive(Clone)]
pub(crate) struct LongLane {
    pub(crate) steps: mpsc::Sender<Command>,
    pub(crate) link: watch::Receiver<Link>,
    pub(crate) turn: Arc<Semaphore>,
}

/// What the open session told the task about the feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Opened {
    /// The epoch its history counts in.
    pub(crate) epoch: u32,
    pub(crate) capabilities: Capabilities,
    /// Its boot count, once its status is read.
    pub(crate) boot_count: Option<u32>,
}

impl Handle {
    pub(crate) fn link(&self) -> Link {
        *self.link.borrow()
    }

    pub(crate) fn epoch(&self) -> Option<u32> {
        self.opened.borrow().map(|opened| opened.epoch)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<MirrorEvent> {
        self.updates.subscribe()
    }

    /// Stop the task and wait until it has ended: its session is closed
    /// and its last update (`offline`) sent. The broadcast closes as this
    /// handle drops.
    pub(crate) async fn stop(self) {
        self.cancel.cancel();
        // The tasks return on cancellation; a panic in one has ended it too.
        let _ = self.task.await;
        if let Some(collector) = self.collector {
            let _ = collector.await;
        }
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
    let (steps, step_receiver) = mpsc::channel(STEPS);
    let (link, watching) = watch::channel(Link::Offline);
    let (opened, opens) = watch::channel(None);
    let (address, addresses) = watch::channel(feeder.address.clone());
    let keys = Keypair::from_secret(feeder.secret);
    let log_stop = log::Stop::default();
    let long = LongLane {
        steps,
        link: watching.clone(),
        turn: Arc::new(Semaphore::new(1)),
    };
    let collector = match shared.config.diagnostics {
        Diagnostics::Read { stats_every } => Some(Collector {
            shared: Arc::clone(&shared),
            id: feeder.id,
            long: long.clone(),
            opened: opens.clone(),
            updates: updates.subscribe(),
            stats_every,
            cancel: cancel.clone(),
        }),
        Diagnostics::Off => None,
    };
    let task = Task {
        zone: shared.zone.subscribe(),
        shared,
        id: feeder.id,
        identity: feeder.identity,
        keys: keys.clone(),
        address: feeder.address,
        reached: address,
        log_stop: Arc::clone(&log_stop),
        commands: receiver,
        steps: step_receiver,
        link,
        opened,
        updates: updates.clone(),
        cancel: cancel.clone(),
        batch: Batch::default(),
    };
    // Every line the task logs, its session's and its commands' included,
    // is written under the feeder's span, so it names the feeder. The span
    // has no parent: a feeder adopted over HTTP would otherwise log under
    // that one request's span for as long as it runs. The collector logs
    // under the same span.
    let span = info_span!(parent: None, "feeder", id = %task.id);
    let collector =
        collector.map(|collector| tasks.spawn(collector.run().instrument(span.clone())));
    let task = tasks.spawn(task.run().instrument(span));
    Handle {
        commands,
        long,
        link: watching,
        opened: opens,
        identity: feeder.identity,
        keys,
        address: addresses,
        log: log::Slot::default(),
        log_stop,
        updates,
        cancel,
        task,
        collector,
    }
}

pub(crate) struct Task {
    pub(crate) shared: Arc<Shared>,
    pub(crate) id: FeederId,
    identity: [u8; 32],
    keys: Keypair,
    address: Option<String>,
    /// Where the feeder was last reached, for the debug log's session.
    reached: watch::Sender<Option<String>>,
    /// Ends the debug log's tail when the feeder has no session left for
    /// this task's.
    log_stop: log::Stop,
    pub(crate) commands: mpsc::Receiver<Command>,
    pub(crate) steps: mpsc::Receiver<Command>,
    /// The household's zone, watched for changes while a session is open.
    pub(crate) zone: watch::Receiver<Option<Arc<Zone>>>,
    link: watch::Sender<Link>,
    pub(crate) opened: watch::Sender<Option<Opened>>,
    pub(crate) updates: broadcast::Sender<MirrorEvent>,
    pub(crate) cancel: CancellationToken,
    /// Samples received and not yet written.
    pub(crate) batch: Batch,
}

impl Task {
    /// Mirror the feeder until the task is stopped, then show it offline,
    /// so a subscriber's last word on a stopped feeder is never `online`.
    async fn run(mut self) {
        self.reconnect().await;
        self.set(Link::Offline);
    }

    async fn reconnect(&mut self) {
        let mut failures = 0u32;
        loop {
            self.set(Link::Connecting);
            let Some(opened) = self.open().await else {
                return;
            };

            let link = match opened {
                Ok((connection, address)) => {
                    info!(%address, build = connection.build(), "connected");
                    self.address = Some(address.to_string());
                    self.reached.send_replace(self.address.clone());
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
                        End::Lost(error) => info!("session ended: {error}"),
                        End::Failed(error) => warn!("session dropped: {error}"),
                    }
                    Link::Offline
                }
                Err(ConnectError::Unreachable(Unreachable(
                    wellfed_connect::Error::UnknownKey,
                ))) => {
                    warn!("the feeder no longer knows this server's key");
                    Link::Refused
                }
                Err(ConnectError::Unreachable(Unreachable(wellfed_connect::Error::Full))) => {
                    // A followed log holds one of the feeder's sessions:
                    // the mirror's own goes first.
                    if let Some(tail) = log::stop(&self.log_stop).take() {
                        info!("every session of the feeder is taken: ending the debug log's");
                        tail.cancel();
                    } else {
                        debug!("connect failed: every session of the feeder is taken");
                    }
                    Link::Offline
                }
                Err(error) => {
                    debug!("connect failed: {error}");
                    Link::Offline
                }
            };
            self.opened.send_replace(None);
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
                step = self.steps.recv() => {
                    step?.offline();
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
                step = self.steps.recv() => {
                    let Some(step) = step else { return false };
                    step.offline();
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
