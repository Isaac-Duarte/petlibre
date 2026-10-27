//! Alerts: raised once per condition, then acknowledged and cleared in
//! place. Each write says whether it changed the row, so the service sends
//! a change on the stream exactly once however many evaluations race.

use jiff::Timestamp;
use wellfed_api::{Acknowledgement, Alert, AlertCondition, AlertId, AlertState, FeederId, UserId};
use uuid::Uuid;

use crate::convert::{from_i64, millis, timestamp, to_i64};
use crate::{Error, Result, Store};

/// An alert about to be raised.
#[derive(Clone, Copy, Debug)]
pub struct NewAlert {
    pub id: AlertId,
    pub feeder: FeederId,
    pub condition: AlertCondition,
    pub since: Timestamp,
    pub opened: Timestamp,
}

/// Which alerts [`Store::alerts`] lists.
#[derive(Clone, Copy, Debug)]
pub struct AlertFilter {
    pub state: Option<AlertState>,
    pub feeder: Option<FeederId>,
    /// Alerts after this `seq`; from the first when `None`.
    pub after: Option<u64>,
    pub limit: u32,
}

/// What [`Store::acknowledge_alert`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acked {
    /// Acknowledged now.
    Now(Alert),
    /// Someone acknowledged it before: as it stands.
    Before(Alert),
    NotFound,
}

impl Store {
    /// Raise `alert`, unless one for the same condition still stands on
    /// that feeder (or, for a feed, was ever raised): the new alert, or
    /// `None` when there was one.
    pub async fn raise_alert(&self, alert: &NewAlert) -> Result<Option<Alert>> {
        let condition = &alert.condition;
        let kind = condition.code();
        let subject = condition.subject();
        let json = serde_json::to_string(condition).map_err(|e| Error::Json("alert", e))?;
        let (since, opened) = (millis(alert.since), millis(alert.opened));

        let raised = sqlx::query_scalar!(
            "INSERT INTO alerts (id, feeder_id, kind, subject, condition, since_ms, opened_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING
             RETURNING seq",
            alert.id.0,
            alert.feeder.0,
            kind,
            subject,
            json,
            since,
            opened,
        )
        .fetch_optional(&self.writer)
        .await?;

        Ok(raised.map(|_| Alert {
            id: alert.id,
            feeder: alert.feeder,
            condition: alert.condition,
            state: AlertState::Open,
            since: alert.since,
            opened: alert.opened,
            acknowledged: None,
            cleared: None,
        }))
    }

    /// Clear an alert at `at`: the alert as it now stands, or `None` when
    /// it was already cleared.
    pub async fn clear_alert(&self, id: AlertId, at: Timestamp) -> Result<Option<Alert>> {
        let at = millis(at);
        let row = sqlx::query_as!(
            Raw,
            r#"UPDATE alerts SET cleared_ms = ? WHERE id = ? AND cleared_ms IS NULL
               RETURNING seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", condition,
                         since_ms, opened_ms, acked_ms, acked_by AS "acked_by?: Uuid", cleared_ms"#,
            at,
            id.0,
        )
        .fetch_optional(&self.writer)
        .await?;
        row.map(|row| row.alert().map(|(_, alert)| alert))
            .transpose()
    }

    /// Acknowledge an alert for `by` at `at`, once.
    pub async fn acknowledge_alert(&self, id: AlertId, by: UserId, at: Timestamp) -> Result<Acked> {
        let at = millis(at);
        let row = sqlx::query_as!(
            Raw,
            r#"UPDATE alerts SET acked_ms = ?, acked_by = ? WHERE id = ? AND acked_ms IS NULL
               RETURNING seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", condition,
                         since_ms, opened_ms, acked_ms, acked_by AS "acked_by?: Uuid", cleared_ms"#,
            at,
            by.0,
            id.0,
        )
        .fetch_optional(&self.writer)
        .await?;

        if let Some(row) = row {
            return Ok(Acked::Now(row.alert()?.1));
        }
        Ok(match self.alert(id).await? {
            Some(alert) => Acked::Before(alert),
            None => Acked::NotFound,
        })
    }

    pub async fn alert(&self, id: AlertId) -> Result<Option<Alert>> {
        let row = sqlx::query_as!(
            Raw,
            r#"SELECT seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", condition,
                      since_ms, opened_ms, acked_ms, acked_by AS "acked_by?: Uuid", cleared_ms
               FROM alerts WHERE id = ?"#,
            id.0,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| row.alert().map(|(_, alert)| alert))
            .transpose()
    }

    /// The feeder's alerts that clear themselves and have not: the ones an
    /// evaluation may find ended. A feed's alert never clears, so it is
    /// left out.
    pub async fn standing_alerts(&self, feeder: FeederId) -> Result<Vec<Alert>> {
        let feed = "feed";
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", condition,
                      since_ms, opened_ms, acked_ms, acked_by AS "acked_by?: Uuid", cleared_ms
               FROM alerts WHERE feeder_id = ? AND cleared_ms IS NULL AND kind != ?
               ORDER BY seq"#,
            feeder.0,
            feed,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| row.alert().map(|(_, alert)| alert))
            .collect()
    }

    /// Up to `filter.limit` alerts after `filter.after`, oldest first, each
    /// with its number.
    pub async fn alerts(&self, filter: AlertFilter) -> Result<Vec<(u64, Alert)>> {
        let after = filter
            .after
            .map_or(Ok(0), |after| to_i64(after, "cursor"))?;
        let feeder = filter.feeder.map(|feeder| feeder.0);
        let state = filter.state.map(AlertState::code);
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", condition,
                      since_ms, opened_ms, acked_ms, acked_by AS "acked_by?: Uuid", cleared_ms
               FROM alerts
               WHERE seq > ?1 AND (?2 IS NULL OR feeder_id = ?2)
                 AND (?3 IS NULL
                      OR (?3 = 'open' AND cleared_ms IS NULL AND acked_ms IS NULL)
                      OR (?3 = 'acknowledged' AND cleared_ms IS NULL AND acked_ms IS NOT NULL)
                      OR (?3 = 'cleared' AND cleared_ms IS NOT NULL))
               ORDER BY seq LIMIT ?4"#,
            after,
            feeder,
            state,
            filter.limit,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter().map(Raw::alert).collect()
    }
}

/// An `alerts` row as SQLite holds it.
struct Raw {
    seq: i64,
    id: Uuid,
    feeder_id: Uuid,
    condition: String,
    since_ms: i64,
    opened_ms: i64,
    acked_ms: Option<i64>,
    acked_by: Option<Uuid>,
    cleared_ms: Option<i64>,
}

impl Raw {
    fn alert(self) -> Result<(u64, Alert)> {
        let condition =
            serde_json::from_str(&self.condition).map_err(|_| Error::Corrupt("alert condition"))?;
        let acknowledged = match (self.acked_ms, self.acked_by) {
            (Some(at), Some(by)) => Some(Acknowledgement {
                by: UserId(by),
                at: timestamp(at)?,
            }),
            (None, None) => None,
            _ => return Err(Error::Corrupt("alert acknowledgement")),
        };
        let cleared = self.cleared_ms.map(timestamp).transpose()?;
        let state = match (cleared, acknowledged) {
            (Some(_), _) => AlertState::Cleared,
            (None, Some(_)) => AlertState::Acknowledged,
            (None, None) => AlertState::Open,
        };

        let alert = Alert {
            id: AlertId(self.id),
            feeder: FeederId(self.feeder_id),
            condition,
            state,
            since: timestamp(self.since_ms)?,
            opened: timestamp(self.opened_ms)?,
            acknowledged,
            cleared,
        };
        Ok((from_i64(self.seq, "alert seq")?, alert))
    }
}
