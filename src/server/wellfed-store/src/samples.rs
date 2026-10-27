//! Live samples: kept raw as the feeder sent them, each with the number
//! its metric is charted by ([`wellfed_api::sample_number`]), and rolled
//! up per UTC hour ([`rollups`]).
//!
//! The raw bytes stay the truth. Ingest hands the store each decoded
//! sample with its bytes, and the store writes its number beside them;
//! when what a number is changes, [`SAMPLE_NUMBER_VERSION`] moves and
//! [`Store::refresh_samples`] decodes every stored sample again at the
//! next start.

mod rollups;

use jiff::Timestamp;
use wellfed_api::{FeederId, MetricKind, SAMPLE_NUMBER_VERSION, SampleValue, sample_number};

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

pub use rollups::HourRow;

/// Stored samples read per query while refreshing.
const BATCH: i64 = 512;

/// One sample to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSample {
    pub received: Timestamp,
    pub uptime_ms: u32,
    pub value: SampleValue,
    /// The value's postcard bytes, as the feeder sent them.
    pub raw: Vec<u8>,
}

/// Where a stored sample sits in a metric's order: when it was received,
/// then the order it was stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SampleKey {
    pub received_ms: i64,
    pub id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRow {
    pub key: SampleKey,
    pub received: Timestamp,
    pub uptime_ms: u32,
    pub metric: MetricKind,
    /// The metric's number, decoded at ingest; `None` when it has none.
    pub number: Option<i64>,
    /// The value's postcard bytes.
    pub value: Vec<u8>,
}

/// A span of receive times: from `from`, up to but not including `to`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub from: Option<Timestamp>,
    pub to: Option<Timestamp>,
}

impl Span {
    fn millis(self) -> (i64, i64) {
        (
            self.from.map_or(i64::MIN, millis),
            self.to.map_or(i64::MAX, millis),
        )
    }
}

/// What [`Store::refresh_samples`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleRefresh {
    /// The numbers were decoded by this version already.
    Current,
    /// Every stored sample's number was decoded again, of which
    /// `undecoded` did not decode and have none; the rollups the raw rows
    /// cover are rebuilt by the next [`Store::roll_up_samples`].
    Decoded { samples: u64, undecoded: u64 },
}

impl Store {
    /// Store a batch of one feeder's samples, in one transaction.
    pub async fn record_samples(&self, id: FeederId, samples: &[NewSample]) -> Result<()> {
        let mut tx = self.writer.begin().await?;
        for sample in samples {
            let received = millis(sample.received);
            let metric = sample.value.metric().code();
            let number = sample_number(&sample.value);
            sqlx::query!(
                "INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, number, value)
                 VALUES (?, ?, ?, ?, ?, ?)",
                id.0,
                received,
                sample.uptime_ms,
                metric,
                number,
                sample.raw,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Up to `limit` samples of `metric` received in `span` after `after`,
    /// oldest first.
    pub async fn samples(
        &self,
        id: FeederId,
        metric: MetricKind,
        span: Span,
        after: Option<SampleKey>,
        limit: u32,
    ) -> Result<Vec<SampleRow>> {
        let code = metric.code();
        let (from, to) = span.millis();
        let after = after.unwrap_or(SampleKey {
            received_ms: i64::MIN,
            id: i64::MIN,
        });
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: i64", received_ms, uptime_ms, number, value FROM samples
               WHERE feeder_id = ?1 AND metric = ?2 AND received_ms >= ?3 AND received_ms < ?4
                 AND (received_ms, id) > (?5, ?6)
               ORDER BY received_ms, id LIMIT ?7"#,
            id.0,
            code,
            from,
            to,
            after.received_ms,
            after.id,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(SampleRow {
                    key: SampleKey {
                        received_ms: row.received_ms,
                        id: row.id,
                    },
                    received: timestamp(row.received_ms)?,
                    uptime_ms: u32::try_from(row.uptime_ms)
                        .map_err(|_| Error::Corrupt("uptime"))?,
                    metric,
                    number: row.number,
                    value: row.value,
                })
            })
            .collect()
    }

    /// Decode every stored sample's number again, with `decode` turning
    /// its bytes back into the value, when the numbers were decoded by
    /// another [`SAMPLE_NUMBER_VERSION`] (or never), and drop the rollups
    /// the raw rows still cover so the next roll-up rebuilds them from the
    /// new numbers. A rollup whose raw rows retention removed is kept as
    /// it was. One transaction on the writer; run it at start, before
    /// ingest begins.
    pub async fn refresh_samples(
        &self,
        decode: impl Fn(&[u8]) -> Option<SampleValue>,
    ) -> Result<SampleRefresh> {
        let mut tx = self.writer.begin().await?;
        let stored = sqlx::query_scalar!("SELECT version FROM sample_marks WHERE id = 0")
            .fetch_one(&mut *tx)
            .await?;
        if stored == i64::from(SAMPLE_NUMBER_VERSION) {
            return Ok(SampleRefresh::Current);
        }

        let (mut samples, mut undecoded, mut after) = (0, 0, 0);
        loop {
            let rows = sqlx::query!(
                r#"SELECT id AS "id!: i64", value FROM samples WHERE id > ? ORDER BY id LIMIT ?"#,
                after,
                BATCH,
            )
            .fetch_all(&mut *tx)
            .await?;
            let Some(last) = rows.last() else {
                break;
            };
            after = last.id;

            for row in rows {
                samples += 1;
                let value = decode(&row.value);
                if value.is_none() {
                    undecoded += 1;
                }
                let number = value.as_ref().and_then(sample_number);
                sqlx::query!("UPDATE samples SET number = ? WHERE id = ?", number, row.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        sqlx::query!(
            "DELETE FROM sample_rollups AS r WHERE EXISTS (
                 SELECT 1 FROM samples AS s
                 WHERE s.feeder_id = r.feeder_id AND s.metric = r.metric
                   AND s.received_ms >= r.hour_ms AND s.received_ms < r.hour_ms + 3600000
             )"
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE sample_marks SET rolled = 0, version = ? WHERE id = 0",
            SAMPLE_NUMBER_VERSION,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(SampleRefresh::Decoded { samples, undecoded })
    }
}
