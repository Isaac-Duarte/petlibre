//! Hourly rollups of the samples' numbers, and the raw retention they
//! outlive.
//!
//! The rollups are folded in by id: `sample_marks.rolled` is the newest
//! sample folded, and each [`Store::roll_up_samples`] adds the samples
//! stored since into their hours' rows, however late a sample arrived for
//! its hour. A read at hour resolution adds the samples past the mark on
//! the fly, so it is whole between roll-ups. Ids are never reused, so the
//! mark stays right after retention empties the table.

use jiff::Timestamp;
use wellfed_api::{FeederId, MetricKind};

use super::Span;
use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

const HOUR_MS: i64 = 3_600_000;

/// Samples folded per transaction, so a long catch-up lets ingest write
/// between its steps.
const ROLL_STEP: i64 = 20_000;

/// Samples deleted per transaction, for the same reason.
const DELETE_STEP: i64 = 10_000;

/// One UTC hour of a metric's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HourRow {
    pub hour: Timestamp,
    pub count: u32,
    pub min: i64,
    pub max: i64,
    pub sum: i64,
}

impl Store {
    /// Fold every sample stored since the last roll-up into its hour's
    /// rollup; how many samples that was.
    pub async fn roll_up_samples(&self) -> Result<u64> {
        let mut folded = 0;
        loop {
            let mut tx = self.writer.begin().await?;
            let rolled = sqlx::query_scalar!("SELECT rolled FROM sample_marks WHERE id = 0")
                .fetch_one(&mut *tx)
                .await?;
            let newest = sqlx::query_scalar!(r#"SELECT MAX(id) AS "newest: i64" FROM samples"#)
                .fetch_one(&mut *tx)
                .await?;
            let Some(newest) = newest.filter(|&newest| newest > rolled) else {
                return Ok(folded);
            };
            let upto = newest.min(rolled.saturating_add(ROLL_STEP));

            sqlx::query!(
                "INSERT INTO sample_rollups (feeder_id, metric, hour_ms, count, min, max, sum)
                 SELECT feeder_id, metric, received_ms / 3600000 * 3600000,
                        COUNT(*), MIN(number), MAX(number), SUM(number)
                 FROM samples
                 WHERE id > ?1 AND id <= ?2 AND number IS NOT NULL
                 GROUP BY 1, 2, 3
                 ON CONFLICT (feeder_id, metric, hour_ms) DO UPDATE SET
                     count = count + excluded.count,
                     min = min(min, excluded.min),
                     max = max(max, excluded.max),
                     sum = sum + excluded.sum",
                rolled,
                upto,
            )
            .execute(&mut *tx)
            .await?;
            let step = sqlx::query_scalar!(
                r#"SELECT COUNT(*) AS "count!: i64" FROM samples WHERE id > ? AND id <= ?"#,
                rolled,
                upto,
            )
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query!("UPDATE sample_marks SET rolled = ? WHERE id = 0", upto)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            folded += u64::try_from(step).map_err(|_| Error::Corrupt("sample count"))?;
        }
    }

    /// Delete the raw samples received before the start of `before`'s
    /// hour that are already in the rollups; how many. Whole hours go, so
    /// a rollup is never left beside part of its raw rows.
    pub async fn delete_samples_before(&self, before: Timestamp) -> Result<u64> {
        let cutoff = millis(before).div_euclid(HOUR_MS).saturating_mul(HOUR_MS);
        let mut deleted = 0;
        loop {
            let done = sqlx::query!(
                "DELETE FROM samples WHERE id IN (
                     SELECT id FROM samples
                     WHERE id <= (SELECT rolled FROM sample_marks WHERE id = 0)
                       AND received_ms < ?
                     ORDER BY id LIMIT ?
                 )",
                cutoff,
                DELETE_STEP,
            )
            .execute(&self.writer)
            .await?
            .rows_affected();
            if done == 0 {
                return Ok(deleted);
            }
            deleted += done;
        }
    }

    /// Up to `limit` hours of `metric` whose start is in `span`, after the
    /// hour starting at `after`, oldest first: the rollups, with the
    /// samples stored since the last roll-up added in.
    pub async fn sample_hours(
        &self,
        id: FeederId,
        metric: MetricKind,
        span: Span,
        after: Option<Timestamp>,
        limit: u32,
    ) -> Result<Vec<HourRow>> {
        let code = metric.code();
        let (from, to) = span.millis();
        // An hour is in when it starts in the span: its samples are those
        // received from the first hour starting at or after `from`.
        let from = ceil_hour(from);
        let to = ceil_hour(to);
        let after = after.map_or(i64::MIN, millis);
        // The samples past the mark are found by id (NOT INDEXED leaves
        // the rowid), since they are the newest few: the metric's index
        // would walk the whole span.
        let rows = sqlx::query!(
            r#"SELECT hour_ms AS "hour_ms!: i64", SUM(count) AS "count!: i64",
                      MIN(min) AS "min!: i64", MAX(max) AS "max!: i64", SUM(sum) AS "sum!: i64"
               FROM (
                   SELECT hour_ms, count, min, max, sum FROM sample_rollups
                   WHERE feeder_id = ?1 AND metric = ?2 AND hour_ms >= ?3 AND hour_ms < ?4
                     AND hour_ms > ?5
                   UNION ALL
                   SELECT received_ms / 3600000 * 3600000 AS hour_ms, COUNT(*) AS count,
                          MIN(number) AS min, MAX(number) AS max, SUM(number) AS sum
                   FROM samples NOT INDEXED
                   WHERE id > (SELECT rolled FROM sample_marks WHERE id = 0)
                     AND feeder_id = ?1 AND metric = ?2 AND number IS NOT NULL
                     AND received_ms >= ?3 AND received_ms < ?4
                     AND received_ms / 3600000 * 3600000 > ?5
                   GROUP BY 1
               )
               GROUP BY hour_ms ORDER BY hour_ms LIMIT ?6"#,
            id.0,
            code,
            from,
            to,
            after,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(HourRow {
                    hour: timestamp(row.hour_ms)?,
                    count: u32::try_from(row.count).map_err(|_| Error::Corrupt("rollup count"))?,
                    min: row.min,
                    max: row.max,
                    sum: row.sum,
                })
            })
            .collect()
    }
}

/// The first hour boundary at or after `ms`, saturating at the ends.
fn ceil_hour(ms: i64) -> i64 {
    let floor = ms.div_euclid(HOUR_MS).saturating_mul(HOUR_MS);
    if floor == ms {
        ms
    } else {
        floor.saturating_add(HOUR_MS)
    }
}
