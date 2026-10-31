//! What the mirror reads from a feeder beside its history: each feed's
//! trace, the latest of each statistics section, and every read of the food
//! section as a typed row. None of it is a projection, so a projection
//! rebuild leaves it alone.

use jiff::Timestamp;
use wellfed_api::{FeederId, FeederRefusal, FoodStats, Stats, StatsSection, TraceSummary};

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

/// A feed's trace as the store keeps it: the summary, and the body in the
/// feeder's own encoding, which the model's `Trace::decode` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTrace {
    pub summary: TraceSummary,
    /// The feeder had it in storage rather than RAM.
    pub stored: bool,
    pub body: Vec<u8>,
}

/// What a trace fetch found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceFound {
    Trace(RawTrace),
    /// The feeder had none, and said why.
    Missing(FeederRefusal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRow {
    pub epoch: u32,
    pub feed: u32,
    pub fetched: Timestamp,
    pub found: TraceFound,
}

/// One section of the statistics as the server last read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsRow {
    pub received: Timestamp,
    pub boot_count: Option<u32>,
    pub stats: Stats,
}

/// One read of the food section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoodReading {
    pub received: Timestamp,
    pub boot_count: Option<u32>,
    pub food: FoodStats,
}

impl Store {
    /// Keep what a fetch of feed `feed`'s trace found. A later fetch of the
    /// same feed replaces it.
    pub async fn record_trace(
        &self,
        id: FeederId,
        epoch: u32,
        feed: u32,
        fetched: Timestamp,
        found: &TraceFound,
    ) -> Result<()> {
        let (summary, stored, body, refusal) = match found {
            TraceFound::Trace(trace) => (
                Some(json(&trace.summary, "trace summary")?),
                Some(trace.stored),
                Some(trace.body.as_slice()),
                None,
            ),
            TraceFound::Missing(refusal) => (None, None, None, Some(json(refusal, "refusal")?)),
        };
        let fetched = millis(fetched);
        sqlx::query!(
            "INSERT INTO feed_traces (feeder_id, epoch, feed, fetched_ms, summary, stored, body, refusal)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (feeder_id, epoch, feed) DO UPDATE SET
                 fetched_ms = excluded.fetched_ms, summary = excluded.summary,
                 stored = excluded.stored, body = excluded.body, refusal = excluded.refusal",
            id.0,
            epoch,
            feed,
            fetched,
            summary,
            stored,
            body,
            refusal,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Feed `feed`'s trace in `epoch`, or in the newest epoch that has one
    /// when `epoch` is `None`.
    pub async fn trace(
        &self,
        id: FeederId,
        epoch: Option<u32>,
        feed: u32,
    ) -> Result<Option<TraceRow>> {
        let row = sqlx::query!(
            r#"SELECT epoch, feed, fetched_ms, summary, stored, body, refusal FROM feed_traces
               WHERE feeder_id = ?1 AND feed = ?2 AND (?3 IS NULL OR epoch = ?3)
               ORDER BY epoch DESC LIMIT 1"#,
            id.0,
            feed,
            epoch,
        )
        .fetch_optional(&self.reader)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        let found = match (row.summary, row.stored, row.body, row.refusal) {
            (Some(summary), Some(stored), Some(body), None) => TraceFound::Trace(RawTrace {
                summary: parse(&summary, "trace summary")?,
                stored: stored != 0,
                body,
            }),
            (None, None, None, Some(refusal)) => TraceFound::Missing(parse(&refusal, "refusal")?),
            _ => return Err(Error::Corrupt("trace")),
        };
        Ok(Some(TraceRow {
            epoch: u32::try_from(row.epoch).map_err(|_| Error::Corrupt("epoch"))?,
            feed: u32::try_from(row.feed).map_err(|_| Error::Corrupt("feed number"))?,
            fetched: timestamp(row.fetched_ms)?,
            found,
        }))
    }

    /// The feeds of `epoch` with no trace fetched yet, newest first, among
    /// the `window` highest-numbered feeds of that epoch: the feeder keeps
    /// only its last few traces, so an older feed is never asked for.
    pub async fn untraced_feeds(&self, id: FeederId, epoch: u32, window: u32) -> Result<Vec<u32>> {
        let rows = sqlx::query_scalar!(
            r#"SELECT f.feed AS "feed!" FROM feeds f
               WHERE f.feeder_id = ?1 AND f.epoch = ?2
                 AND f.feed > (SELECT MAX(feed) FROM feeds WHERE feeder_id = ?1 AND epoch = ?2) - ?3
                 AND NOT EXISTS (
                     SELECT 1 FROM feed_traces t
                     WHERE t.feeder_id = ?1 AND t.epoch = ?2 AND t.feed = f.feed
                 )
               ORDER BY f.feed DESC"#,
            id.0,
            epoch,
            window,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|feed| u32::try_from(feed).map_err(|_| Error::Corrupt("feed number")))
            .collect()
    }

    /// Keep a read of one statistics section as its latest, and a food
    /// section also as a typed row of its own, in one transaction.
    pub async fn record_stats(
        &self,
        id: FeederId,
        received: Timestamp,
        boot_count: Option<u32>,
        stats: &Stats,
    ) -> Result<()> {
        let section = section_code(stats);
        let body = json(stats, "statistics")?;
        let received = millis(received);
        let mut tx = self.writer.begin().await?;
        sqlx::query!(
            "INSERT INTO feeder_stats (feeder_id, section, received_ms, boot_count, stats)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (feeder_id, section) DO UPDATE SET received_ms = excluded.received_ms,
                 boot_count = excluded.boot_count, stats = excluded.stats",
            id.0,
            section,
            received,
            boot_count,
            body,
        )
        .execute(&mut *tx)
        .await?;
        if let Stats::Food(food) = stats {
            sqlx::query!(
                "INSERT INTO food_readings
                     (feeder_id, received_ms, boot_count, present, ran_low, refilled, feeds_since_refill)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                id.0,
                received,
                boot_count,
                food.present,
                food.ran_low,
                food.refilled,
                food.feeds_since_refill,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The latest read of each statistics section, in the sections' order.
    pub async fn latest_stats(&self, id: FeederId) -> Result<Vec<StatsRow>> {
        let rows = sqlx::query!(
            "SELECT received_ms, boot_count, stats FROM feeder_stats WHERE feeder_id = ?",
            id.0,
        )
        .fetch_all(&self.reader)
        .await?;

        let mut read = rows
            .into_iter()
            .map(|row| {
                Ok(StatsRow {
                    received: timestamp(row.received_ms)?,
                    boot_count: boot(row.boot_count)?,
                    stats: parse(&row.stats, "statistics")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        read.sort_by_key(|row| {
            StatsSection::ALL
                .iter()
                .position(|section| *section == row.stats.section())
        });
        Ok(read)
    }

    /// The food section's reads received in `from..to`, oldest first.
    pub async fn food_readings(
        &self,
        id: FeederId,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<FoodReading>> {
        let (from, to) = (millis(from), millis(to));
        let rows = sqlx::query!(
            "SELECT received_ms, boot_count, present, ran_low, refilled, feeds_since_refill
             FROM food_readings WHERE feeder_id = ? AND received_ms >= ? AND received_ms < ?
             ORDER BY received_ms, rowid",
            id.0,
            from,
            to,
        )
        .fetch_all(&self.reader)
        .await?;

        let count = |value: i64| u32::try_from(value).map_err(|_| Error::Corrupt("food count"));
        rows.into_iter()
            .map(|row| {
                Ok(FoodReading {
                    received: timestamp(row.received_ms)?,
                    boot_count: boot(row.boot_count)?,
                    food: FoodStats {
                        present: row.present.map(|present| present != 0),
                        ran_low: count(row.ran_low)?,
                        refilled: count(row.refilled)?,
                        feeds_since_refill: row.feeds_since_refill.map(count).transpose()?,
                    },
                })
            })
            .collect()
    }
}

/// The section's stored name, its JSON spelling.
const fn section_code(stats: &Stats) -> &'static str {
    match stats.section() {
        StatsSection::Counters => "counters",
        StatsSection::Memory => "memory",
        StatsSection::Flash => "flash",
        StatsSection::Power => "power",
        StatsSection::Food => "food",
        StatsSection::Feeds => "feeds",
        StatsSection::Portions => "portions",
    }
}

fn boot(value: Option<i64>) -> Result<Option<u32>> {
    value
        .map(|boot| u32::try_from(boot).map_err(|_| Error::Corrupt("boot count")))
        .transpose()
}

fn json<T: serde::Serialize>(value: &T, what: &'static str) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::Json(what, error))
}

fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &'static str) -> Result<T> {
    serde_json::from_str(json).map_err(|_| Error::Corrupt(what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sections_code_is_its_json_spelling() {
        for section in StatsSection::ALL {
            let json = serde_json::to_string(&section).unwrap();
            let stats = match section {
                StatsSection::Counters => Stats::Counters(Default::default()),
                StatsSection::Memory => Stats::Memory(Default::default()),
                StatsSection::Flash => Stats::Flash(Default::default()),
                StatsSection::Power => Stats::Power(Default::default()),
                StatsSection::Food => Stats::Food(Default::default()),
                StatsSection::Feeds => Stats::Feeds(Default::default()),
                StatsSection::Portions => Stats::Portions(Default::default()),
            };
            assert_eq!(json, format!("\"{}\"", section_code(&stats)));
            assert_eq!(stats.section(), section);
        }
    }
}
