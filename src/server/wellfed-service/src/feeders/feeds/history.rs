//! Settling feeds from the feeder's history, never by asking again.
//!
//! The feeder numbers each feed by its journal id, which keeps counting
//! across restarts, and records every reserved feed's end as a `Fed`
//! event: a feed a restart interrupted is resolved at the next boot as
//! `uncertain` under its own number, by the feeder. The history is read
//! from the store's feeds projection, one row per feed. So:
//!
//! - **A feed answered with its number** is settled by the `Fed` event
//!   with that number.
//! - **A feed whose answer was lost** has no number. It can only have
//!   ended in a `Fed` event after the history the server held when it was
//!   sent, by the server's key, or by the feeder as `uncertain` (an
//!   interrupted one), whose number no other entry holds. Another client's
//!   feed, or the panel's, is never it. It is settled by that event only
//!   when the match is unambiguous: it is the feeder's one unknown feed
//!   without a number, none is in flight (whose answer could claim the
//!   event), and exactly one event fits. Otherwise it stays unknown, as it
//!   does when the journal holds no history position for it. An
//!   `unrecorded` one does not count against it: its own late `Fed` event
//!   is the rare case, and counting it would keep every later lost answer
//!   unknown.
//! - **A feed the history never settles** is `unrecorded` once the feeder
//!   has restarted since it was sent (its boot count changed), has opened
//!   its journal and resolved any interrupted feed (its readiness is not
//!   `starting`, `resolving` or `feeding`), and the server holds its whole
//!   history (through the live status's newest event, in the live
//!   session's epoch, with no gap and no `lost` events after the feed): a
//!   feed it had reserved would be there. It never ran, or its record was
//!   lost; either way it is never resent. A `Fed` event found later still
//!   replaces it, when it is the feeder's only feed without a number.

use std::sync::Arc;

use tracing::{info, warn};
use uuid::Uuid;
use wellfed_api::{
    Blocked, CommandOutcome, CommandState, EventKind, FeedOutcome, FeederId, Readiness, Source,
};
use wellfed_model::{Answer, Op};
use wellfed_store::{CommandRow, Feed, Projected, Synced};

use crate::{Result, Service, lock};

/// Where a feed's result can be in the history: after event `after` of
/// `epoch`, the newest the server held when the feed was sent.
#[derive(Clone, Copy)]
struct Since {
    epoch: u32,
    after: u64,
}

impl Since {
    fn of(entry: &CommandRow) -> Option<Self> {
        entry.history.map(|held| Self {
            epoch: held.epoch,
            after: held.seq,
        })
    }
}

/// Whether a `Fed` event can be the server's own feed: by the server's
/// key, or by the feeder. The feeder names itself only for a feed a
/// restart interrupted, which the next boot resolves as `uncertain`, so a
/// feeder's event with any other outcome is no client's feed. A plan's
/// feed is never a client's, even one a restart interrupted.
fn by_us(fed: &Feed, us: Option<[u8; 4]>) -> bool {
    match fed.by {
        Source::Client { client } => Some(client.0) == us,
        Source::Feeder => fed.outcome == FeedOutcome::Uncertain,
        Source::Panel | Source::Schedule { .. } => false,
    }
}

fn fed(fed: &Feed) -> CommandOutcome {
    CommandOutcome::Fed {
        feed: fed.feed,
        requested: fed.requested,
        completed: fed.completed,
        outcome: fed.outcome,
        reason: fed.reason,
    }
}

impl Service {
    /// [`Self::reconcile`], logging a failure: the next event tries again.
    pub(crate) async fn reconcile_best_effort(&self, id: FeederId) {
        if let Err(error) = self.reconcile(id).await {
            warn!(%error, "the feeds could not be settled from the history yet");
        }
    }

    /// Settle what the feeder's history now says of its open feeds.
    async fn reconcile(&self, id: FeederId) -> Result<()> {
        // One at a time per feeder: its watcher and a feed's answer both
        // call this.
        let turn = Arc::clone(lock(&self.feeders.resolving).entry(id).or_default());
        let _turn = turn.lock().await;
        let store = &self.shared.store;
        let open = store.open_feeds(id).await?;
        if open.is_empty() {
            return Ok(());
        }
        let Some(current) = store.feeder(id).await?.and_then(|row| row.synced) else {
            return Ok(());
        };
        let us = self.fingerprint(id);
        let in_flight = open.iter().any(|entry| entry.state == CommandState::Sent);
        let numberless = |state: Option<CommandState>| {
            open.iter()
                .filter(|entry| entry.state != CommandState::Sent && entry.feed.is_none())
                .filter(|entry| state.is_none_or(|state| entry.state == state))
                .count()
        };
        let (unknowns, all) = (numberless(Some(CommandState::Unknown)), numberless(None));

        let mut silent = Vec::new();
        for entry in open
            .iter()
            .filter(|entry| entry.state != CommandState::Sent)
        {
            let since = Since::of(entry);
            let found = match entry.feed {
                Some(number) => {
                    let epoch = since.map_or(current.epoch, |since| since.epoch);
                    store.feed_result(id, epoch, number).await?
                }
                // Nothing to match it against: it stays unknown.
                None if since.is_none() => continue,
                None => {
                    let alone = match entry.state {
                        CommandState::Unknown => unknowns == 1,
                        _ => all == 1,
                    };
                    match self.unclaimed(id, since, us).await?.as_slice() {
                        [] => None,
                        [only] if alone && !in_flight => Some(*only),
                        // Ambiguous: it stays as it is.
                        _ => continue,
                    }
                }
            };

            match (found, since) {
                (Some(found), _) => self.settle(entry, &fed(&found.value)).await?,
                (None, Some(since)) if entry.outcome != Some(CommandOutcome::Unrecorded) => {
                    silent.push((entry, since));
                }
                (None, _) => {}
            }
        }

        if !silent.is_empty() && !in_flight {
            self.unrecorded(id, &silent).await?;
        }
        Ok(())
    }

    /// The feeds after `since` that can be the server's own and that no
    /// journal entry holds yet.
    async fn unclaimed(
        &self,
        id: FeederId,
        since: Option<Since>,
        us: Option<[u8; 4]>,
    ) -> Result<Vec<Projected<Feed>>> {
        let Some(since) = since else {
            return Ok(Vec::new());
        };
        let store = &self.shared.store;
        let mut fits = Vec::new();
        for found in store.feeds_after(id, since.epoch, since.after).await? {
            if by_us(&found.value, us)
                && !store
                    .feed_claimed(id, since.epoch, found.value.feed)
                    .await?
            {
                fits.push(found);
            }
        }
        Ok(fits)
    }

    /// Settle `entry` with `outcome`, and tell the stream.
    async fn settle(&self, entry: &CommandRow, outcome: &CommandOutcome) -> Result<()> {
        let now = self.shared.clock.now();
        if self
            .shared
            .store
            .resolve_command(&entry.key, outcome, now)
            .await?
        {
            info!(key = %Uuid::from_bytes(entry.key), ?outcome, "a feed is settled from the history");
            self.published(&entry.key).await;
        }
        Ok(())
    }

    /// Settle as `unrecorded` the feeds in `silent` (with where their result
    /// would be) that the feeder's history can no longer settle.
    async fn unrecorded(&self, id: FeederId, silent: &[(&CommandRow, Since)]) -> Result<()> {
        let store = &self.shared.store;
        let restarted =
            |entry: &CommandRow, boot: u32| entry.boot_count.is_some_and(|sent| sent != boot);
        let Some(held) = store.feeder(id).await?.and_then(|row| row.synced) else {
            return Ok(());
        };
        // The status read at each connect shows a restart first, and the
        // newest event it named must be held before a live read can help.
        let cached = store.state(id).await?.status.map(|(status, _)| status);
        let Some(cached) = cached else {
            return Ok(());
        };
        let behind = held.seq < cached.last_event.unwrap_or(0);
        if behind
            || !silent
                .iter()
                .any(|(entry, _)| restarted(entry, cached.boot_count))
        {
            return Ok(());
        }

        let Ok(Answer::Status(status)) = self.mirror.ask(id, self.request_id()?, Op::Status).await
        else {
            return Ok(());
        };
        store
            .record_status(id, &status, self.shared.clock.now())
            .await?;
        let opened = match status.readiness {
            Some(Readiness::Ready) => true,
            Some(Readiness::Blocked(blocked)) => !matches!(
                blocked,
                Blocked::Starting | Blocked::Resolving | Blocked::Feeding
            ),
            None => false,
        };
        // Held through the live session's newest event, in its epoch.
        let held = store.feeder(id).await?.and_then(|row| row.synced);
        let live = self.mirror.epoch(id);
        let whole =
            |held: Synced| Some(held.epoch) == live && held.seq >= status.last_event.unwrap_or(0);
        let Some(held) = held.filter(|&held| opened && whole(held)) else {
            return Ok(());
        };

        for &(entry, since) in silent {
            if !restarted(entry, status.boot_count) || since.epoch != held.epoch {
                continue;
            }
            if self.incomplete(id, since).await? {
                continue;
            }
            self.settle(entry, &CommandOutcome::Unrecorded).await?;
        }
        Ok(())
    }

    /// Whether the history since `since` may be missing a feed's result:
    /// a gap, or events the feeder could not record.
    async fn incomplete(&self, id: FeederId, since: Since) -> Result<bool> {
        let store = &self.shared.store;
        let after = Some(since.after);
        let gaps = store
            .gaps(id, since.epoch, after, i64::MAX.unsigned_abs())
            .await?;
        if !gaps.is_empty() {
            return Ok(true);
        }
        let lost = store
            .events(id, since.epoch, after, 1, Some(EventKind::Lost))
            .await?;
        Ok(!lost.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use wellfed_api::{FeedReason, Hex};

    use super::*;

    fn feed(by: Source, outcome: FeedOutcome) -> Feed {
        Feed {
            feed: 1,
            by,
            requested: 1,
            completed: 0,
            outcome,
            reason: FeedReason::Output,
        }
    }

    /// The server's key, or the feeder resolving an interrupted feed; never
    /// another client, the panel, or the feeder with a result of its own.
    #[test]
    fn only_the_servers_feeds_and_interrupted_ones_can_be_ours() {
        let us = Some([1, 2, 3, 4]);
        let client = |key| Source::Client { client: Hex(key) };

        assert!(by_us(&feed(client([1, 2, 3, 4]), FeedOutcome::Done), us));
        assert!(!by_us(&feed(client([9, 9, 9, 9]), FeedOutcome::Done), us));
        assert!(!by_us(&feed(client([1, 2, 3, 4]), FeedOutcome::Done), None));
        assert!(by_us(&feed(Source::Feeder, FeedOutcome::Uncertain), us));
        assert!(!by_us(&feed(Source::Feeder, FeedOutcome::Done), us));
        assert!(!by_us(&feed(Source::Panel, FeedOutcome::Uncertain), us));
    }
}
