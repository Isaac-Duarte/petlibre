//! Live samples, kept raw as the feeder sent them.

use jiff::Timestamp;
use wellfed_api::{FeederId, MetricKind};

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

pub struct NewSample<'a> {
    pub received: Timestamp,
    pub uptime_ms: u32,
    pub metric: MetricKind,
    /// The metric's postcard bytes.
    pub value: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRow {
    pub received: Timestamp,
    pub uptime_ms: u32,
    pub metric: MetricKind,
    pub value: Vec<u8>,
}

impl Store {
    pub async fn record_sample(&self, id: FeederId, sample: NewSample<'_>) -> Result<()> {
        let received = millis(sample.received);
        let metric = sample.metric.code();
        sqlx::query!(
            "INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, value) VALUES (?, ?, ?, ?, ?)",
            id.0,
            received,
            sample.uptime_ms,
            metric,
            sample.value,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Up to `limit` samples of `metric`, newest first.
    pub async fn samples(
        &self,
        id: FeederId,
        metric: MetricKind,
        limit: u32,
    ) -> Result<Vec<SampleRow>> {
        let code = metric.code();
        let rows = sqlx::query!(
            "SELECT received_ms, uptime_ms, value FROM samples
             WHERE feeder_id = ? AND metric = ? ORDER BY received_ms DESC, rowid DESC LIMIT ?",
            id.0,
            code,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(SampleRow {
                    received: timestamp(row.received_ms)?,
                    uptime_ms: u32::try_from(row.uptime_ms)
                        .map_err(|_| Error::Corrupt("uptime"))?,
                    metric,
                    value: row.value,
                })
            })
            .collect()
    }
}
