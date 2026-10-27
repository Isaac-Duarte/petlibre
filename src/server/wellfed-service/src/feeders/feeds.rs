//! Feeds and their cancels, through the journal like every write.
//!
//! A feed is answered once the feeder has reserved it in its own journal,
//! with its number, and that answer is the route's: the feed runs on. Its
//! result is the feeder's `Fed` event, which settles the entry
//! (`history.rs`) and goes on the stream. A feed whose outcome is unknown
//! is never sent again, under its key or any other: the feeder forgets
//! request ids at a restart, so a resend could feed twice. It is settled
//! only from the history.

use wellfed_api::{Command, CommandKind, CommandOutcome, FeedRequest, FeederId};
use wellfed_mirror::MirrorEvent;
use wellfed_model::Op;
use serde::Serialize;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info_span};

use crate::feeders::commands::done;
use crate::feeders::journal::{Write, view};
use crate::{Caller, Error, IdempotencyKey, InternalError, Resource, Result, Service};

mod history;

/// A cancel as it is hashed into the journal.
#[derive(Serialize)]
struct CancelFeed {
    feed: u32,
}

impl Service {
    /// Ask the feeder for a feed of `portions`, under the caller's key: the
    /// journal entry once the feeder has reserved it, its number in
    /// `feed`. A retry under the same key answers from the entry, which by
    /// then may hold the feed's result; it never feeds again.
    pub async fn feed(
        &self,
        caller: &Caller,
        id: FeederId,
        portions: u8,
        key: IdempotencyKey,
    ) -> Result<Command> {
        self.live_row(id).await?;

        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Feed,
                body: &FeedRequest { portions },
                key: Some(key),
                op: Op::Feed { portions },
            })
            .await?;

        match recorded.outcome {
            CommandOutcome::Feeding { .. }
            | CommandOutcome::Fed { .. }
            | CommandOutcome::Unrecorded => {
                let row = self.shared.store.command(&recorded.key).await?;
                row.map(view)
                    .ok_or_else(|| Error::NotFound(Resource::Command))
            }
            CommandOutcome::Refused { refusal } => Err(Error::Refused(refusal)),
            CommandOutcome::Done
            | CommandOutcome::Settings { .. }
            | CommandOutcome::Invited { .. } => Err(InternalError::Unexpected.into()),
        }
    }

    /// Stop the feed numbered `feed` if it is still running. Its result is
    /// still its `Fed` event, `cancelled` with the portions it gave. One
    /// that has ended is refused `not_found`. A cancel whose outcome was
    /// unknown is resent under its request id, and `not_found` then is no
    /// failure: the first attempt may have stopped the feed, or it ended on
    /// its own; its `Fed` event says which.
    pub async fn cancel_feed(
        &self,
        caller: &Caller,
        id: FeederId,
        feed: u32,
        key: Option<IdempotencyKey>,
    ) -> Result<()> {
        self.live_row(id).await?;

        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Cancel,
                body: &CancelFeed { feed },
                key,
                op: Op::CancelFeed { feed },
            })
            .await?;

        done(&recorded.outcome)
    }

    /// Settle the feeder's feeds from its history whenever it may hold
    /// something new: a `Fed` event, any other event or gap (a sync after
    /// a reconnect moves the history on), the session coming up, or the
    /// broadcast lagging past what it held.
    pub(crate) fn watch_feeds(&self, id: FeederId, mut updates: Receiver<MirrorEvent>) {
        let service = self.clone();
        let cancel: CancellationToken = self.cancel.child_token();
        let span = info_span!(parent: None, "feeder", id = %id);
        let watching = async move {
            loop {
                let update = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    update = updates.recv() => update,
                };
                let settle = match update {
                    Ok(MirrorEvent::Event(_) | MirrorEvent::Gap(_) | MirrorEvent::Link(_)) => true,
                    Ok(MirrorEvent::Sample(_)) => false,
                    Err(RecvError::Lagged(_)) => {
                        updates = updates.resubscribe();
                        true
                    }
                    Err(RecvError::Closed) => return,
                };
                if settle {
                    service.reconcile_best_effort(id).await;
                }
            }
        };
        self.tasks.spawn(watching.instrument(span));
    }
}
