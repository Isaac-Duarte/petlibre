//! One feeder's connection task: it owns the session, mirrors the feeder's
//! history into the store and runs the commands the service hands it.
//!
//! Each connect runs in this order:
//! 1. subscribe after the newest event the store holds, so everything the
//!    feeder recorded while the server was away arrives first (the session
//!    starts from the oldest event kept when the feeder's ring is a new
//!    one, under a new epoch);
//! 2. set the feeder's clock;
//! 3. read its status and settings into the store's cache.
//!
//! Then it serves commands, events and samples until the session ends, and
//! sets the clock again every [`Timing::time_sync`]. An event is stored
//! (moving the cursor) before it is published, and a store failure ends the
//! session, so the reconnect replays whatever was not stored.
//!
//! [`Timing::time_sync`]: crate::Timing::time_sync

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{Connection as Link, FeederId, StreamEvent};
use wellfed_connect::{Answer, Connection, Cursor, Message, Resume};
use wellfed_core::noise::Keypair;
use wellfed_proto::api::{FeederMessage, Op, Outcome};
use wellfed_store::{FeederRow, NewEvent, NewSample};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, warn};

use crate::{Discovery, Error, RequestId, Result, Shared, convert, keys};

/// Commands waiting for the task; more wait in `send`.
const COMMANDS: usize = 16;
/// How long the task waits on a quiet feeder before looking again; the
/// connection keeps the session alive meanwhile.
const QUIET: Duration = Duration::from_secs(60);
/// Room for an encoded event body (16 bytes on the feeder) or sample.
const RAW: usize = 32;

/// The service's side of a task.
pub(crate) struct Handle {
    pub(crate) commands: mpsc::Sender<Command>,
    connection: watch::Receiver<Link>,
}

impl Handle {
    pub(crate) fn connection(&self) -> Link {
        *self.connection.borrow()
    }
}

/// One request for the feeder, and where its answer goes.
pub(crate) struct Command {
    pub(crate) id: RequestId,
    pub(crate) op: Op<'static>,
    pub(crate) reply: oneshot::Sender<Result<Answer>>,
}

/// Why a session ended.
enum End {
    Cancelled,
    Lost(wellfed_connect::Error),
    Failed(Error),
}

pub(crate) fn spawn(
    shared: Arc<Shared>,
    row: &FeederRow,
    cancel: CancellationToken,
    tasks: &TaskTracker,
) -> Result<Handle> {
    let secret = keys::open(&shared.config.key, row.id, &row.identity, &row.key)?;
    let (commands, receiver) = mpsc::channel(COMMANDS);
    let (link, connection) = watch::channel(Link::Offline);
    let task = Task {
        shared,
        id: row.id,
        identity: row.identity,
        keys: Keypair::from_secret(secret),
        address: row.address.clone(),
        commands: receiver,
        link,
        cancel,
    };
    tasks.spawn(task.run());
    Ok(Handle {
        commands,
        connection,
    })
}

struct Task {
    shared: Arc<Shared>,
    id: FeederId,
    identity: [u8; 32],
    keys: Keypair,
    address: Option<String>,
    commands: mpsc::Receiver<Command>,
    link: watch::Sender<Link>,
    cancel: CancellationToken,
}

impl Task {
    async fn run(mut self) {
        let mut failures = 0u32;
        loop {
            self.set(Link::Connecting);
            let open = open(
                Arc::clone(&self.shared),
                self.id,
                self.identity,
                self.keys.clone(),
                self.address.clone(),
            );
            tokio::pin!(open);
            let opened = loop {
                tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => return,
                    command = self.commands.recv() => {
                        let Some(command) = command else { return };
                        let _ = command.reply.send(Err(Error::Offline));
                    }
                    opened = &mut open => break opened,
                }
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
                Err(Error::Unreachable(wellfed_connect::Error::UnknownKey)) => {
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

    /// Serve one open session until it ends. The feeder shows online only
    /// once the connect sequence is done, so no command waits behind it.
    async fn serve(&mut self, mut connection: Connection) -> End {
        let started = tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(End::Cancelled),
            started = self.start(&mut connection) => started,
        };
        if let Err(end) = started {
            return end;
        }
        self.set(Link::Online);

        let every = self.shared.config.timing.time_sync;
        let mut next_sync = Instant::now() + every;
        loop {
            let step = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(End::Cancelled),
                command = self.commands.recv() => match command {
                    Some(command) => tokio::select! {
                        biased;
                        () = self.cancel.cancelled() => Err(End::Cancelled),
                        step = run(&mut connection, command) => step,
                    },
                    None => Err(End::Cancelled),
                },
                () = tokio::time::sleep_until(next_sync) => {
                    next_sync += every;
                    self.sync_time(&mut connection).await
                }
                message = connection.next(QUIET) => match message {
                    Ok(message) => self.apply(connection.epoch(), &message).await,
                    Err(wellfed_connect::Error::Timeout) => Ok(()),
                    Err(error) => Err(End::Lost(error)),
                },
            };
            if let Err(end) = step {
                return end;
            }
        }
    }

    /// The connect sequence: history first, then the clock, then the
    /// cached status and settings.
    async fn start(&self, connection: &mut Connection) -> Result<(), End> {
        let row = self.shared.store.feeder(self.id).await.map_err(fail)?;
        let Some(row) = row else {
            return Err(End::Cancelled);
        };
        let from = row.synced.map(|synced| Cursor {
            epoch: synced.epoch,
            seq: synced.seq,
        });
        let resume = connection.subscribe(from, true).await.map_err(End::Lost)?;
        if resume == Resume::Restarted {
            warn!(
                feeder = %self.id,
                epoch = connection.epoch(),
                "the feeder's history is a new one; mirroring it from its oldest event"
            );
        }
        self.sync_time(connection).await?;

        let answer = self.ask(connection, Op::Status).await?;
        if let Outcome::Status(status) = answer.outcome() {
            let status = convert::status(&status);
            let now = self.shared.clock.now();
            self.shared
                .store
                .record_status(self.id, &status, now)
                .await
                .map_err(fail)?;
        }
        let answer = self.ask(connection, Op::GetSettings).await?;
        if let Outcome::Settings(settings) = answer.outcome() {
            let now = self.shared.clock.now();
            self.shared
                .store
                .record_settings(self.id, &convert::settings(settings), now)
                .await
                .map_err(fail)?;
        }
        Ok(())
    }

    /// Set the feeder's clock to this server's, in the configured zone.
    async fn sync_time(&self, connection: &mut Connection) -> Result<(), End> {
        let now = self.shared.clock.now();
        let sync = wellfed_client::time::sync(now, &self.shared.config.zone);
        match self.ask(connection, Op::TimeSync(sync)).await?.outcome() {
            Outcome::Done { .. } => debug!(feeder = %self.id, "clock set"),
            Outcome::Refused(refusal) => {
                warn!(feeder = %self.id, "the feeder refused the clock: {refusal:?}");
            }
            other => warn!(feeder = %self.id, "unexpected answer to the clock: {other:?}"),
        }
        Ok(())
    }

    /// One request of the task's own.
    async fn ask(&self, connection: &mut Connection, op: Op<'_>) -> Result<Answer, End> {
        let id = self.shared.random().map_err(End::Failed)?;
        connection.ask(id, op).await.map_err(End::Lost)
    }

    /// Store what the feeder pushed, then publish it.
    async fn apply(&self, epoch: u32, message: &Message) -> Result<(), End> {
        let store = &self.shared.store;
        let received = self.shared.clock.now();
        let mut raw = [0u8; RAW];
        match message.get() {
            FeederMessage::Event(event) => {
                let len = wellfed_proto::encode(&event.body, &mut raw)
                    .map_err(|_| End::Failed(Error::Unexpected))?;
                let event = convert::event(epoch, &event, received);
                let new = store
                    .record_event(
                        self.id,
                        NewEvent {
                            epoch,
                            seq: event.seq,
                            at: event.at,
                            received,
                            kind: event.body.kind(),
                            body: &raw[..len],
                        },
                    )
                    .await
                    .map_err(fail)?;
                if new {
                    self.shared.hub.publish(StreamEvent::Event {
                        feeder: self.id,
                        event,
                    });
                }
            }
            FeederMessage::Gap { from, to } => {
                warn!(feeder = %self.id, from, to, "events lost before the server read them");
                store
                    .record_gap(self.id, epoch, from, to, received)
                    .await
                    .map_err(fail)?;
                self.shared.hub.publish(StreamEvent::Gap {
                    feeder: self.id,
                    gap: wellfed_api::Gap {
                        epoch,
                        from,
                        to,
                        received,
                    },
                });
            }
            FeederMessage::Sample(sample) => {
                let len = wellfed_proto::encode(&sample.metric, &mut raw)
                    .map_err(|_| End::Failed(Error::Unexpected))?;
                let sample = convert::sample(sample, received);
                store
                    .record_sample(
                        self.id,
                        NewSample {
                            received,
                            uptime_ms: sample.uptime_ms,
                            metric: sample.value.metric(),
                            value: &raw[..len],
                        },
                    )
                    .await
                    .map_err(fail)?;
                self.shared.hub.publish(StreamEvent::Sample {
                    feeder: self.id,
                    sample,
                });
            }
            // Answers go to `ask`; `next` yields only what the feeder pushed.
            FeederMessage::Response { .. } => {}
        }
        Ok(())
    }

    fn set(&self, link: Link) {
        let changed = self.link.send_if_modified(|current| {
            let changed = *current != link;
            *current = link;
            changed
        });
        if changed {
            self.shared.hub.publish(StreamEvent::Connection {
                feeder: self.id,
                connection: link,
            });
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
                    let _ = command.reply.send(Err(Error::Offline));
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

/// Run one service command on the session. A command whose answer does not
/// arrive is unknown, not failed: the feeder may have run it. One whose
/// caller stopped waiting is dropped unsent, so the caller's `Unknown`
/// never turns into a late write.
async fn run(connection: &mut Connection, command: Command) -> Result<(), End> {
    if command.reply.is_closed() {
        return Ok(());
    }
    match connection.ask(command.id, command.op).await {
        Ok(answer) => {
            let _ = command.reply.send(Ok(answer));
            Ok(())
        }
        Err(wellfed_connect::Error::Timeout) => {
            let _ = command.reply.send(Err(Error::Unknown));
            Ok(())
        }
        // Refused before anything was sent: the session is untouched.
        Err(wellfed_connect::Error::ReservedId | wellfed_connect::Error::Encode) => {
            let _ = command.reply.send(Err(Error::Unexpected));
            Ok(())
        }
        Err(error) => {
            let _ = command.reply.send(Err(Error::Unknown));
            Err(End::Lost(error))
        }
    }
}

/// Open a session at the stored address, or, when that fails and discovery
/// is on, wherever mDNS finds the feeder now.
async fn open(
    shared: Arc<Shared>,
    id: FeederId,
    identity: [u8; 32],
    keys: Keypair,
    address: Option<String>,
) -> Result<(Connection, SocketAddr)> {
    let stored = address.as_deref().and_then(|address| {
        wellfed_connect::locate(&identity, Some(address), Duration::ZERO).ok()
    });
    if let Some(address) = stored {
        let ephemeral = shared.random()?;
        match Connection::open(&keys, identity, address, ephemeral).await {
            Ok(connection) => return Ok((connection, address)),
            // The feeder answered: looking elsewhere would not help.
            Err(
                error @ (wellfed_connect::Error::UnknownKey
                | wellfed_connect::Error::Version(_)
                | wellfed_connect::Error::Full),
            ) => return Err(Error::Unreachable(error)),
            Err(error) if shared.config.discovery == Discovery::Off => {
                return Err(Error::Unreachable(error));
            }
            Err(error) => debug!(feeder = %id, "{address}: {error}; looking over mDNS"),
        }
    } else if shared.config.discovery == Discovery::Off {
        return Err(Error::Unreachable(wellfed_connect::Error::Address(
            address.unwrap_or_default(),
        )));
    }

    let found = discover(identity).await?;
    if let Err(error) = shared.store.set_address(id, &found.to_string()).await {
        warn!(feeder = %id, "remember the address {found}: {error}");
    }
    let ephemeral = shared.random()?;
    let connection = Connection::open(&keys, identity, found, ephemeral)
        .await
        .map_err(Error::Unreachable)?;
    Ok((connection, found))
}

/// The feeder's address over mDNS, which blocks a thread while it browses.
pub(crate) async fn discover(identity: [u8; 32]) -> Result<SocketAddr> {
    tokio::task::spawn_blocking(move || {
        wellfed_connect::locate(&identity, None, wellfed_connect::DISCOVERY)
    })
    .await
    .map_err(|_| Error::Unexpected)?
    .map_err(Error::Unreachable)
}

fn fail(error: wellfed_store::Error) -> End {
    End::Failed(Error::Store(error))
}
