//! One open session, from the connect sequence until it ends.
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
//! sets the clock again every [`Timing::time_sync`]. The feeder shows online
//! only once the connect sequence is done, so no command waits behind it.
//!
//! [`Timing::time_sync`]: crate::Timing::time_sync

use std::time::Duration;

use wellfed_api::Connection as Link;
use wellfed_connect::{Connection, Cursor, Resume};
use wellfed_model::{Answer, Op};
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::RandomFailed;
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
    /// Serve one open session until it ends.
    pub(crate) async fn serve(&mut self, mut connection: Connection) -> End {
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
                        step = command.run(&mut connection) => step,
                    },
                    None => Err(End::Cancelled),
                },
                () = tokio::time::sleep_until(next_sync) => {
                    next_sync += every;
                    self.sync_time(&mut connection).await
                }
                message = connection.next(QUIET) => match message {
                    Ok(pushed) => self.apply(pushed).await,
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
        let Some(row) = self.shared.store.feeder(self.id).await? else {
            return Err(End::Cancelled);
        };
        let from = row.synced.map(|synced| Cursor {
            epoch: synced.epoch,
            seq: synced.seq,
        });
        let resume = connection.subscribe(from, true).await.map_err(End::Lost)?;
        if resume == Resume::Restarted {
            warn!(
                epoch = connection.epoch(),
                "the feeder's history is a new one; mirroring it from its oldest event"
            );
        }
        self.sync_time(connection).await?;

        if let Answer::Status(status) = self.ask(connection, &Op::Status).await? {
            let now = self.shared.clock.now();
            self.shared
                .store
                .record_status(self.id, &status, now)
                .await?;
        }
        if let Answer::Settings(settings) = self.ask(connection, &Op::GetSettings).await? {
            let now = self.shared.clock.now();
            self.shared
                .store
                .record_settings(self.id, &settings, now)
                .await?;
        }
        Ok(())
    }

    /// Set the feeder's clock to this server's, in the configured zone.
    async fn sync_time(&self, connection: &mut Connection) -> Result<(), End> {
        let now = self.shared.clock.now();
        let sync = wellfed_client::time::sync(now, &self.shared.config.zone);
        match self.ask(connection, &Op::TimeSync(sync)).await? {
            Answer::Done { .. } => debug!("clock set"),
            Answer::Refused(refusal) => {
                warn!("the feeder refused the clock: {refusal:?}");
            }
            other => warn!("unexpected answer to the clock: {other:?}"),
        }
        Ok(())
    }

    /// One request of the task's own.
    async fn ask(&self, connection: &mut Connection, op: &Op) -> Result<Answer, End> {
        let id = self.shared.random()?;
        connection.ask(id, op).await.map_err(End::Lost)
    }
}
