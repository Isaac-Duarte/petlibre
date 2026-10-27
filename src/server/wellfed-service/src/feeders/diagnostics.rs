//! What the mirror reads from a feeder beside its history: each feed's
//! trace and the latest statistics, as stored, and the debug log an owner
//! follows live, never stored.

use jiff::Timestamp;
use wellfed_api::{
    FeedTrace, FeederId, FeederStats, LogEvent, StatsRead, TraceQuery, TraceRecord,
};
use wellfed_mirror::LogError;
use wellfed_model::{LogLines, Trace};
use wellfed_store::{FoodReading, TraceFound};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use crate::{CommandError, Error, FeederError, InternalError, Resource, Result, Service};

/// A followed debug log: its batches in order, until the tail ends.
#[derive(Debug)]
pub struct LogFollow(Receiver<LogLines>);

impl LogFollow {
    /// The next batch, a note that this follower fell behind, or `None`
    /// once the tail has ended.
    pub async fn next(&mut self) -> Option<LogEvent> {
        match self.0.recv().await {
            Ok(LogLines {
                seq,
                dropped,
                lines,
            }) => Some(LogEvent::Lines {
                seq,
                dropped,
                lines,
            }),
            Err(RecvError::Lagged(batches)) => Some(LogEvent::Lagged { batches }),
            Err(RecvError::Closed) => None,
        }
    }
}

impl Service {
    /// Feed `feed`'s trace as the server fetched it after the feed.
    pub async fn feed_trace(
        &self,
        id: FeederId,
        feed: u32,
        query: TraceQuery,
    ) -> Result<FeedTrace> {
        self.row(id).await?;

        let row = self.shared.store.trace(id, query.epoch, feed).await?;
        let row = row.ok_or(Error::NotFound(Resource::FeedTrace))?;
        let record = match row.found {
            TraceFound::Trace(raw) => {
                // The mirror stores only a trace that decodes.
                let trace = Trace::decode(raw.summary, raw.stored, &raw.body)
                    .map_err(|_| wellfed_store::Error::Corrupt("trace"))?;
                TraceRecord::Stored { trace }
            }
            TraceFound::Missing(refusal) => TraceRecord::Missing { refusal },
        };
        Ok(FeedTrace {
            epoch: row.epoch,
            feed: row.feed,
            fetched: row.fetched,
            record,
        })
    }

    /// The latest read of each statistics section.
    pub async fn stats(&self, id: FeederId) -> Result<FeederStats> {
        self.row(id).await?;

        let rows = self.shared.store.latest_stats(id).await?;
        let sections = rows
            .into_iter()
            .map(|row| StatsRead {
                section: row.stats.section(),
                received: row.received,
                boot_count: row.boot_count,
                stats: row.stats,
            })
            .collect();
        Ok(FeederStats { sections })
    }

    /// The food section's reads received in `from..to`, oldest first: the
    /// food beam as the feeder last read it, for the food views and alerts.
    pub async fn food_readings(
        &self,
        id: FeederId,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<FoodReading>> {
        Ok(self.shared.store.food_readings(id, from, to).await?)
    }

    /// Follow the feeder's debug log live. The server's key must be an
    /// owner's on the feeder; on another the feeder refuses `forbidden`.
    pub async fn follow_log(&self, id: FeederId) -> Result<LogFollow> {
        self.row(id).await?;

        let lines = self.mirror.follow_log(id).await?;
        Ok(LogFollow(lines))
    }
}

/// A log refusal in the service's domains. Nothing a tail sends changes
/// the feeder, so none of these is uncertain.
impl From<LogError> for Error {
    fn from(error: LogError) -> Self {
        match error {
            LogError::NotRunning => Error::NotFound(Resource::Feeder),
            LogError::Offline => CommandError::Offline.into(),
            LogError::Connect(error) => error.into(),
            LogError::Refused(refusal) => Error::Refused(refusal.into()),
            LogError::Unanswered => FeederError::LogUnanswered.into(),
            LogError::Unexpected => InternalError::Unexpected.into(),
        }
    }
}
