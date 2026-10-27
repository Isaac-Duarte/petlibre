//! The samples the server keeps, raw or by the hour, and the jobs that
//! keep their rollups and drop raw rows past the owner's retention.

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{FeederId, HourSample, RawSample, Resolution, SamplePage, SampleQuery};
use wellfed_mirror::stored_sample;
use wellfed_store::{SampleKey, Span};
use tracing::info;

use crate::{FeederError, Invalid, Result, Service};

/// Samples or hours per page, unless asked.
const PAGE: u32 = 100;
const PAGE_MAX: u32 = 1000;

impl Service {
    /// A page of one feeder's samples of one metric, oldest first.
    pub async fn samples(&self, id: FeederId, query: SampleQuery) -> Result<SamplePage> {
        let limit = query.limit.unwrap_or(PAGE);
        if !(1..=PAGE_MAX).contains(&limit) {
            return Err(Invalid::PageLimit.into());
        }
        if let (Some(from), Some(to)) = (query.from, query.to)
            && from >= to
        {
            return Err(Invalid::SampleSpan.into());
        }

        let span = Span {
            from: query.from,
            to: query.to,
        };
        let resolution = query.resolution.unwrap_or_default();
        let after = query.after.as_deref();
        let (raw_after, hour_after) = match resolution {
            Resolution::Raw => (after.map(raw_cursor).transpose()?, None),
            Resolution::Hour => (None, after.map(hour_cursor).transpose()?),
        };
        if self.shared.store.feeder(id).await?.is_none() {
            return Err(FeederError::NotFound.into());
        }

        // One more than the page, to know whether another follows.
        let ask = limit + 1;
        let page = match resolution {
            Resolution::Raw => {
                let mut rows = self
                    .shared
                    .store
                    .samples(id, query.metric, span, raw_after, ask)
                    .await?;
                let next = more(&mut rows, limit)
                    .map(|last| format!("{}.{}", last.key.received_ms, last.key.id));
                let items = rows
                    .into_iter()
                    .map(|row| {
                        let value =
                            stored_sample(&row.value).ok_or(FeederError::UndecodableSample)?;
                        Ok(RawSample {
                            received: row.received,
                            uptime_ms: row.uptime_ms,
                            value,
                            number: row.number,
                        })
                    })
                    .collect::<Result<_>>()?;
                SamplePage::Raw { items, next }
            }
            Resolution::Hour => {
                let mut rows = self
                    .shared
                    .store
                    .sample_hours(id, query.metric, span, hour_after, ask)
                    .await?;
                let next =
                    more(&mut rows, limit).map(|last| last.hour.as_millisecond().to_string());
                let items = rows
                    .into_iter()
                    .map(|row| HourSample {
                        hour: row.hour,
                        count: row.count,
                        min: row.min,
                        max: row.max,
                        mean: mean(row.sum, row.count),
                    })
                    .collect();
                SamplePage::Hour { items, next }
            }
        };
        Ok(page)
    }

    /// Fold the samples stored since the last run into their hours'
    /// rollups. Every hour the server was down is filled in by the first
    /// run after, since the rollups follow what was stored, not the clock.
    pub async fn run_rollups(&self, now: Timestamp) -> Result<()> {
        let _ = now;
        let folded = self.shared.store.roll_up_samples().await?;
        info!("{folded} samples rolled up");
        Ok(())
    }

    /// Drop the raw samples older than the owner's retention at `now`,
    /// in whole hours, after rolling up whatever is not yet: the hourly
    /// rollups are kept forever. Nothing goes while no retention is set.
    pub async fn run_retention(&self, now: Timestamp) -> Result<()> {
        let Some(days) = self.shared.config.raw_sample_days else {
            return Ok(());
        };
        let kept = SignedDuration::from_hours(i64::from(days.get()).saturating_mul(24));
        let Ok(before) = now.checked_sub(kept) else {
            return Ok(());
        };

        self.shared.store.roll_up_samples().await?;
        let deleted = self.shared.store.delete_samples_before(before).await?;
        info!("{deleted} raw samples older than {days} days deleted");
        Ok(())
    }
}

/// Trim `rows` to `limit`, returning the last kept when more followed.
fn more<T>(rows: &mut Vec<T>, limit: u32) -> Option<&T> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    if rows.len() <= limit {
        return None;
    }
    rows.truncate(limit);
    rows.last()
}

fn mean(sum: i64, count: u32) -> f64 {
    // An hour's sum of device readings is far inside f64's exact
    // integers (2^53), so the conversion loses nothing.
    sum as f64 / f64::from(count.max(1))
}

/// A raw page's cursor: `<received ms>.<id>`, as this server gave it.
fn raw_cursor(text: &str) -> Result<SampleKey, Invalid> {
    let (received, id) = text.split_once('.').ok_or(Invalid::PageCursor)?;
    let parse = |part: &str| part.parse::<i64>().map_err(|_| Invalid::PageCursor);
    Ok(SampleKey {
        received_ms: parse(received)?,
        id: parse(id)?,
    })
}

/// An hourly page's cursor: the last hour's start, in Unix milliseconds.
fn hour_cursor(text: &str) -> Result<Timestamp, Invalid> {
    let ms = text.parse::<i64>().map_err(|_| Invalid::PageCursor)?;
    Timestamp::from_millisecond(ms).map_err(|_| Invalid::PageCursor)
}
