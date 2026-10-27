//! One open session, from the connect sequence until it ends.
//!
//! Each connect runs in this order:
//! 1. subscribe after the newest event the store holds, so everything the
//!    feeder recorded while the server was away arrives first (the session
//!    starts from the oldest event kept when the feeder's ring is a new
//!    one, under a new epoch);
//! 2. set the feeder's clock, in the household's zone;
//! 3. give the feeder the zone's POSIX rule, if its time settings hold
//!    another, so it follows DST on its own;
//! 4. read its status and settings into the store's cache.
//!
//! Then it serves commands on their lanes ([`crate::lanes`]), events and
//! samples until the session ends, and sets the clock and checks the rule
//! again every [`Timing::time_sync`] and whenever the household's zone
//! changes. A feeder offline at the change
//! gets the new zone in its next connect sequence. The feeder shows online
//! only once the connect sequence is done, so no command waits behind it.
//!
//! [`Timing::time_sync`]: crate::Timing::time_sync

use std::time::Duration;

use jiff::tz::TimeZone;
use wellfed_api::Connection as Link;
use wellfed_connect::{Connection, Cursor, Received, Resume};
use wellfed_model::{Answer, Op, Refusal, TimeChanges};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::RandomFailed;
use crate::lanes::{Lane, Lanes};
use crate::task::Task;

/// How long the task waits on a quiet feeder before looking again; the
/// connection keeps the session alive meanwhile.
const QUIET: Duration = Duration::from_secs(60);

/// Why a session ended.
pub(crate) enum End {
    Cancelled,
    Lost(wellfed_connect::Error),
    Failed(Failure),
}

/// A failure of the server's own that ends a session, so the reconnect
/// replays whatever was not stored.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    #[error(transparent)]
    Store(#[from] wellfed_store::Error),
    #[error(transparent)]
    Random(#[from] RandomFailed),
    #[error("the feeder sent a value that does not encode")]
    Encode,
}

impl From<wellfed_store::Error> for End {
    fn from(error: wellfed_store::Error) -> Self {
        Self::Failed(error.into())
    }
}

impl From<RandomFailed> for End {
    fn from(error: RandomFailed) -> Self {
        Self::Failed(error.into())
    }
}

impl Task {
    /// Serve one open session until it ends. Whatever ends it, each
    /// command still in flight is unknown, and the samples left in the
    /// batch are written.
    pub(crate) async fn serve(&mut self, mut connection: Connection) -> End {
        let mut lanes = Lanes::default();
        let end = self.serve_lanes(&mut connection, &mut lanes).await;
        lanes.lost();
        if let Err(End::Failed(failure)) = self.write_samples().await {
            warn!("write the session's last samples: {failure}");
        }
        end
    }

    async fn serve_lanes(&mut self, connection: &mut Connection, lanes: &mut Lanes) -> End {
        let cancel = self.cancel.clone();
        let started = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(End::Cancelled),
            started = self.start(connection, lanes) => started,
        };
        if let Err(end) = started {
            return end;
        }
        self.set(Link::Online);

        let every = self.shared.config.timing.time_sync;
        let mut next_sync = Instant::now() + every;
        // The sender lives as long as the mirror; should it go, the zone
        // simply stops changing.
        let mut zone_open = true;
        loop {
            let short_free = lanes.short_free();
            let long_free = lanes.long_free(!self.commands.is_empty());
            let samples_due = self.batch.due();
            let step = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(End::Cancelled),
                () = sleep_until_some(samples_due), if samples_due.is_some() => {
                    self.write_samples().await
                }
                command = self.commands.recv(), if short_free => match command {
                    Some(command) => lanes.dispatch(Lane::Short, command, connection),
                    None => Err(End::Cancelled),
                },
                step = self.steps.recv(), if long_free => match step {
                    Some(step) => lanes.dispatch(Lane::Long, step, connection),
                    None => Err(End::Cancelled),
                },
                () = tokio::time::sleep_until(next_sync) => {
                    next_sync += every;
                    self.keep_time(connection, lanes).await
                }
                changed = self.zone.changed(), if zone_open => match changed {
                    Ok(()) => self.keep_time(connection, lanes).await,
                    Err(_) => {
                        zone_open = false;
                        Ok(())
                    }
                },
                received = connection.receive(QUIET) => match received {
                    Ok(Received::Pushed(pushed)) => self.apply(pushed).await,
                    Ok(Received::Answer { id, answer }) => lanes.answered(id, answer, connection),
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
    async fn start(&mut self, connection: &mut Connection, lanes: &mut Lanes) -> Result<(), End> {
        let Some(row) = self.shared.store.feeder(self.id).await? else {
            return Err(End::Cancelled);
        };
        let from = row.synced.map(|synced| Cursor {
            epoch: synced.epoch,
            seq: synced.seq,
        });
        let resume = connection.subscribe(from, true).await.map_err(End::Lost)?;
        self.epoch.send_replace(Some(connection.epoch()));
        if resume == Resume::Restarted {
            warn!(
                epoch = connection.epoch(),
                "the feeder's history is a new one; mirroring it from its oldest event"
            );
        }
        self.keep_time(connection, lanes).await?;

        if let Answer::Status(status) = self.read(connection, lanes, &Op::Status).await? {
            let now = self.shared.clock.now();
            self.shared
                .store
                .record_status(self.id, &status, now)
                .await?;
        }
        if let Answer::Settings(settings) = self.read(connection, lanes, &Op::GetSettings).await? {
            let now = self.shared.clock.now();
            self.shared
                .store
                .record_settings(self.id, &settings, now)
                .await?;
        }
        Ok(())
    }

    /// Set the feeder's clock and its zone's rule.
    async fn keep_time(
        &mut self,
        connection: &mut Connection,
        lanes: &mut Lanes,
    ) -> Result<(), End> {
        self.sync_time(connection, lanes).await?;
        self.push_zone(connection, lanes).await
    }

    /// Set the feeder's clock to this server's, in the household's zone.
    async fn sync_time(
        &mut self,
        connection: &mut Connection,
        lanes: &mut Lanes,
    ) -> Result<(), End> {
        let now = self.shared.clock.now();
        // Seen here, so a change already applied does not fire again.
        let zone = self.zone.borrow_and_update().clone();
        let zone = zone
            .as_deref()
            .map_or(TimeZone::UTC, |zone| zone.tz().clone());
        let sync = wellfed_client::time::sync(now, &zone);
        match self.ask(connection, lanes, &Op::TimeSync(sync)).await? {
            None => {
                warn!(feeder = %self.id, "no answer to the clock in time; set at the next sync")
            }
            Some(Answer::Done { .. }) => debug!("clock set"),
            Some(Answer::Refused(refusal)) => {
                warn!("the feeder refused the clock: {refusal:?}");
            }
            Some(other) => warn!("unexpected answer to the clock: {other:?}"),
        }
        Ok(())
    }

    /// Give the feeder the household zone's rule when its time settings
    /// hold another. Nothing here is a feed: a write that is lost or
    /// refused is tried again at the next sync or connect, under a new id,
    /// from the time settings read then.
    async fn push_zone(
        &mut self,
        connection: &mut Connection,
        lanes: &mut Lanes,
    ) -> Result<(), End> {
        let zone = self.zone.borrow_and_update().clone();
        let Some((zone, rule)) = zone.as_deref().and_then(|z| Some((z, z.rule()?))) else {
            return Ok(());
        };

        let view = match self.ask(connection, lanes, &Op::GetTime).await? {
            None => {
                warn!(feeder = %self.id, "no answer to the time settings in time; read at the next sync");
                return Ok(());
            }
            Some(Answer::Time(view)) => view,
            Some(Answer::Refused(refusal)) => {
                debug!(feeder = %self.id, "the feeder serves no time settings: {refusal:?}");
                return Ok(());
            }
            Some(other) => {
                warn!(feeder = %self.id, "unexpected answer to the time settings: {other:?}");
                return Ok(());
            }
        };
        if view.zone.as_deref() == Some(rule) {
            return Ok(());
        }

        let put = Op::PutTime {
            base: view.generation,
            changes: TimeChanges {
                zone: Some(rule.to_owned()),
                ntp: None,
            },
        };
        match self.ask(connection, lanes, &put).await? {
            None => {
                warn!(feeder = %self.id, "no answer to the zone in time; checked at the next sync")
            }
            Some(Answer::Done { .. }) => {
                info!(feeder = %self.id, zone = zone.name(), rule, "zone set")
            }
            Some(Answer::Refused(Refusal::Forbidden)) => info!(
                feeder = %self.id,
                "the server's key may not set the feeder's zone; its clock syncs carry the \
                 zone's next change"
            ),
            Some(Answer::Refused(refusal)) => {
                warn!(feeder = %self.id, "the feeder refused the zone: {refusal:?}");
            }
            Some(other) => warn!(feeder = %self.id, "unexpected answer to the zone: {other:?}"),
        }
        Ok(())
    }

    /// A read of the connect sequence: with no command in flight yet, no
    /// answer in time means the feeder is not serving, and ends the session.
    async fn read(
        &mut self,
        connection: &mut Connection,
        lanes: &mut Lanes,
        op: &Op,
    ) -> Result<Answer, End> {
        self.ask(connection, lanes, op)
            .await?
            .ok_or(End::Lost(wellfed_connect::Error::Timeout))
    }

    /// One request of the task's own, and its answer: `None` when none came
    /// in time. The commands in flight meanwhile are answered and what the
    /// feeder pushes is stored, as the loop would; no new command goes out
    /// until this is answered. The feeder answers in order, so this one
    /// may wait behind a slow write on a lane; running out of time then does
    /// not end the session, which would make the lanes' writes in flight
    /// unknown. Its late answer is dropped.
    async fn ask(
        &mut self,
        connection: &mut Connection,
        lanes: &mut Lanes,
        op: &Op,
    ) -> Result<Option<Answer>, End> {
        let id = self.shared.random()?;
        connection.request(id, op).map_err(End::Lost)?;
        loop {
            match connection.receive(QUIET).await {
                Ok(Received::Answer {
                    id: answered,
                    answer,
                }) if answered == id => return Ok(answer),
                Ok(Received::Answer { id, answer }) => lanes.answered(id, answer, connection)?,
                Ok(Received::Pushed(pushed)) => self.apply(pushed).await?,
                // The session's own answer timer reports it unanswered.
                Err(wellfed_connect::Error::Timeout) => {}
                Err(error) => return Err(End::Lost(error)),
            }
        }
    }
}

/// Sleep until `at`; `None` is never selected (its branch is disabled), so
/// it returns at once.
async fn sleep_until_some(at: Option<Instant>) {
    if let Some(at) = at {
        tokio::time::sleep_until(at).await;
    }
}
