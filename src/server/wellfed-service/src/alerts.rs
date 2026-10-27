//! Alerts: the rules (`rules.rs`) run over what the server holds of each
//! feeder, and each alert raised, acknowledged or cleared is recorded and
//! sent on the stream once.
//!
//! An evaluation runs for a feeder when something it reads may have
//! changed (its link, an event, a journal entry settled, an install ending,
//! a refill recorded) and for every feeder once a minute, for the rules
//! that turn on time passing. Evaluations take turns, so two never decide
//! from different reads at once; the store's writes say whether they
//! changed anything, so nothing is streamed twice.
//!
//! Delivery to phones is not here: the stream carries each change, and
//! the app's push reads it.

mod rules;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod timed;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    Alert, AlertId, AlertPage, AlertQuery, Connection as Link, FeederId, PageQuery, StreamEvent,
    User,
};
use wellfed_mirror::MirrorEvent;
use wellfed_store::{Acked, AlertFilter, NewAlert};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tracing::{Instrument, info_span, warn};

use crate::page::{self, after_seq};
use crate::{AlertError, Result, Service, lock};

use rules::{Decision, Facts};

/// How often every feeder is evaluated, for the rules time turns on.
const TICK: Duration = Duration::from_secs(60);
/// How far back a feed's result is looked for: past any one evaluation's
/// reach, and short enough to read on every tick.
const FEED_LOOKBACK: SignedDuration = SignedDuration::from_hours(24);

/// What the alert evaluations share.
#[derive(Debug, Default)]
pub(crate) struct Alerting {
    /// Since when each feeder has had no session, by the server's clock.
    offline: Mutex<HashMap<FeederId, Timestamp>>,
    /// Held by the one evaluation running.
    turn: tokio::sync::Mutex<()>,
}

impl Alerting {
    /// Note the feeder's link at `now`: an offline spell starts at the
    /// first link that is not online and ends at the next that is.
    pub(crate) fn link(&self, feeder: FeederId, link: Link, now: Timestamp) {
        let mut offline = lock(&self.offline);
        if link == Link::Online {
            offline.remove(&feeder);
        } else {
            offline.entry(feeder).or_insert(now);
        }
    }

    fn offline_since(&self, feeder: FeederId) -> Option<Timestamp> {
        lock(&self.offline).get(&feeder).copied()
    }
}

impl Service {
    /// A page of alerts, oldest first.
    pub async fn alerts(&self, query: AlertQuery) -> Result<AlertPage> {
        let paging = PageQuery {
            after: query.after,
            limit: query.limit,
        };
        let limit = page::limit(&paging)?;
        let after = after_seq(&paging)?;

        let rows = self
            .shared
            .store
            .alerts(AlertFilter {
                state: query.state,
                feeder: query.feeder,
                after,
                limit: limit + 1,
            })
            .await?;
        let (rows, next) = page::split(rows, limit, |(seq, _)| seq.to_string());
        Ok(AlertPage {
            items: rows.into_iter().map(|(_, alert)| alert).collect(),
            next,
        })
    }

    /// Acknowledge an alert for `user`. One acknowledged before stays as
    /// it was, and is answered as it stands.
    pub async fn acknowledge_alert(&self, user: &User, id: AlertId) -> Result<Alert> {
        let now = self.shared.clock.now();
        match self
            .shared
            .store
            .acknowledge_alert(id, user.id, now)
            .await?
        {
            Acked::Now(alert) => {
                self.shared.hub.publish(StreamEvent::Alert { alert });
                Ok(alert)
            }
            Acked::Before(alert) => Ok(alert),
            Acked::NotFound => Err(AlertError::NotFound.into()),
        }
    }

    /// Evaluate the feeder's alerts soon, in the background: something they
    /// read changed.
    pub(crate) fn alerts_changed(&self, feeder: FeederId) {
        let service = self.clone();
        self.tasks.spawn(async move {
            let now = service.shared.clock.now();
            service.evaluate_best_effort(feeder, now).await;
        });
    }

    /// Follow the feeder's mirror broadcast: note its link, and evaluate
    /// whenever its link or history moves.
    pub(crate) fn watch_alerts(&self, id: FeederId, mut updates: Receiver<MirrorEvent>) {
        let service = self.clone();
        let cancel = self.cancel.child_token();
        let span = info_span!(parent: None, "feeder", id = %id);
        let watching = async move {
            loop {
                let update = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    update = updates.recv() => update,
                };
                let now = service.shared.clock.now();
                match update {
                    Ok(MirrorEvent::Link(link)) => service.alerting.link(id, link, now),
                    Ok(MirrorEvent::Event(_) | MirrorEvent::Gap(_)) => {}
                    Ok(MirrorEvent::Sample(_)) => continue,
                    Err(RecvError::Lagged(_)) => {
                        updates = updates.resubscribe();
                        if let Some(link) = service.mirror.link(id) {
                            service.alerting.link(id, link, now);
                        }
                    }
                    Err(RecvError::Closed) => return,
                }
                service.evaluate_best_effort(id, now).await;
            }
        };
        self.tasks.spawn(watching.instrument(span));
    }

    /// Evaluate every feeder once a minute, by the service's clock.
    pub(crate) fn tick_alerts(&self) {
        let service = self.clone();
        let cancel = self.cancel.child_token();
        let ticking = async move {
            loop {
                let next = service
                    .shared
                    .clock
                    .now()
                    .checked_add(TICK)
                    .unwrap_or(Timestamp::MAX);
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    () = service.shared.clock.sleep_until(next) => {}
                }

                let feeders = match service.shared.store.feeders().await {
                    Ok(rows) => rows,
                    Err(error) => {
                        warn!("list the feeders for their alerts: {error}");
                        continue;
                    }
                };
                let now = service.shared.clock.now();
                for row in feeders {
                    service.evaluate_best_effort(row.id, now).await;
                }
            }
        };
        self.tasks
            .spawn(ticking.instrument(info_span!(parent: None, "alerts")));
    }

    async fn evaluate_best_effort(&self, feeder: FeederId, now: Timestamp) {
        if let Err(error) = self.evaluate_alerts(feeder, now).await {
            warn!(%feeder, "evaluate the alerts: {error}");
        }
    }

    /// Raise what holds for the feeder at `now`, and clear what ended.
    pub(crate) async fn evaluate_alerts(&self, feeder: FeederId, now: Timestamp) -> Result<()> {
        let _turn = self.alerting.turn.lock().await;

        let facts = self.facts(feeder, now).await?;
        let standing = self.shared.store.standing_alerts(feeder).await?;
        let Decision { raise, clear } = rules::decide(&facts, &standing, now);

        for held in raise {
            let alert = NewAlert {
                id: AlertId(self.shared.new_id()?),
                feeder,
                condition: held.condition,
                since: held.since,
                opened: now,
            };
            if let Some(alert) = self.shared.store.raise_alert(&alert).await? {
                self.shared.hub.publish(StreamEvent::Alert { alert });
            }
        }
        for id in clear {
            if let Some(alert) = self.shared.store.clear_alert(id, now).await? {
                self.shared.hub.publish(StreamEvent::Alert { alert });
            }
        }
        Ok(())
    }

    /// What the rules read of the feeder, as the store and the links hold
    /// it now.
    async fn facts(&self, feeder: FeederId, now: Timestamp) -> Result<Facts> {
        let store = &self.shared.store;
        let link = self.links().get(&feeder).copied().unwrap_or(Link::Offline);
        let lately = now.checked_sub(FEED_LOOKBACK).unwrap_or(now);

        Ok(Facts {
            link,
            offline_since: self.alerting.offline_since(feeder),
            feeds: store.feeds(feeder, lately, Timestamp::MAX).await?,
            unrecorded: store.unrecorded_feeds(feeder).await?,
            reading: store.last_food_reading(feeder).await?,
            refilled: store.last_refill(feeder).await?.map(|refill| refill.at),
            install: store.latest_install(feeder).await?,
            boot: store.last_boot(feeder).await?,
            sync: store.last_clock_sync(feeder).await?,
            status: store.state(feeder).await?.status,
            blocked: store.blocked_since_free(feeder).await?,
        })
    }
}
