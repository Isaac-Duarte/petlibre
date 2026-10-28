//! The command journal: every write the server sends a feeder is recorded
//! before it is sent, under its idempotency key, and its answer after.
//!
//! The feeder dedups writes by device request id, but only the last 32
//! per client, and every user shares the server's key: a retry that
//! arrives after 32 other writes would run again. So a retry reads the
//! journal first (`retry.rs`):
//!
//! - An answered write returns its recorded outcome and never reaches the
//!   feeder.
//! - A `sent` write is in flight: the start marks every leftover `sent`
//!   entry unknown, so inside a running server only the attempt that made
//!   the entry sends it. A retry meanwhile is answered unknown, unsent.
//! - An unknown write goes out again under the request id it was first sent
//!   under, which the feeder answers from memory if it ran it, but only for
//!   a kind whose resend rule allows it. A feed never goes out again.
//!   A settings change resent after the feeder's ring has moved on runs
//!   again against its base, which the first run moved past, so it comes
//!   back as a stale refusal although the change was made.
//!
//! A key that already names another request is refused and nothing is
//! sent. The send runs in a task of its own, so the entry reaches its
//! answer or `unknown` even when the caller stops waiting. Every change to
//! an entry goes on the stream. A feed's entry is settled later from the
//! feeder's history (`feeds/history.rs`).

use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;
use wellfed_api::{
    Command, CommandKind, CommandOutcome, CommandPage, CommandQuery, CommandState, FeederId,
    StreamEvent, User,
};
use wellfed_formats::canonical::canonical_of;
use wellfed_mirror::AskError;
use wellfed_model::{Answer, Invite, Op};
use wellfed_store::{CommandFilter, CommandRow, Journaled, NewCommand, Synced};

use self::id::write_id;
use self::retry::{Retry, retry};
use crate::{
    Caller, CommandError, Error, IdempotencyKey, InternalError, Invalid, Permission, RequestId,
    Resource, Result, Service, can,
};

mod id;
mod retry;

const PAGE: u32 = 100;
const PAGE_MAX: u32 = 1000;
/// Past every stored row number (SQLite integers are `i64`).
const CURSOR_MAX: u64 = i64::MAX.unsigned_abs();

/// A write to send through the journal.
pub(crate) struct Write<'a, B> {
    /// Who sent it, and the token it came through.
    pub(crate) caller: &'a Caller,
    pub(crate) feeder: FeederId,
    pub(crate) kind: CommandKind,
    /// The request as the client sent it: its canonical hash tells a
    /// retry from a key reused for something else.
    pub(crate) body: &'a B,
    /// The client's key; the server draws one when it sent none.
    pub(crate) key: Option<IdempotencyKey>,
    pub(crate) op: Op,
}

/// A write's journal key and the feeder's answer, fresh or recorded.
pub(crate) struct Recorded {
    pub(crate) key: IdempotencyKey,
    pub(crate) outcome: CommandOutcome,
    /// An invite's secret, from a fresh answer only: the journal never
    /// holds it.
    pub(crate) invite: Option<Invite>,
}

/// Whether this attempt made the entry: only then may a send that never
/// left drop it.
#[derive(Clone, Copy)]
enum Attempt {
    First,
    Retry,
}

impl Service {
    /// One entry: the caller's own, or anyone's for an owner. Another
    /// user's entry is not found, so a member learns nothing of it.
    pub async fn command(&self, user: &User, key: IdempotencyKey) -> Result<Command> {
        let row = self.shared.store.command(&key).await?;
        match row {
            Some(row) if can(user.role, Permission::AllHistory) || row.user == user.id => {
                Ok(view(row))
            }
            _ => Err(Error::NotFound(Resource::Command)),
        }
    }

    /// A page of the journal, oldest first: the caller's own entries, or
    /// everyone's for an owner.
    pub async fn commands(&self, user: &User, query: CommandQuery) -> Result<CommandPage> {
        let limit = query.limit.unwrap_or(PAGE);
        if !(1..=PAGE_MAX).contains(&limit) {
            return Err(Invalid::CommandLimit.into());
        }
        let after = query
            .after
            .map(|after| match after.parse::<u64>() {
                Ok(after) if after <= CURSOR_MAX => Ok(after),
                _ => Err(Invalid::CommandCursor),
            })
            .transpose()?;

        // One more than the page shows whether another page follows.
        let mut rows = self
            .shared
            .store
            .commands(CommandFilter {
                user: (!can(user.role, Permission::AllHistory)).then_some(user.id),
                feeder: query.feeder,
                state: query.state,
                after,
                limit: limit + 1,
            })
            .await?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);

        let next = more
            .then(|| rows.last().map(|row| row.seq.to_string()))
            .flatten();
        Ok(CommandPage {
            items: rows.into_iter().map(view).collect(),
            next,
        })
    }

    /// Send `write` through the journal: record it, or find its key's
    /// entry, then answer from the entry or send it.
    pub(crate) async fn journaled<B: Serialize>(&self, write: Write<'_, B>) -> Result<Recorded> {
        let key = match write.key {
            Some(key) => key,
            None => uuid::Builder::from_random_bytes(self.shared.random()?)
                .into_uuid()
                .into_bytes(),
        };
        let body = canonical_of(write.body).map_err(InternalError::Body)?;
        let body_hash: [u8; 32] = Sha256::digest(body).into();
        let request = write_id(&write.caller.user, write.feeder, write.kind, &key);
        let boot_count = self
            .shared
            .store
            .state(write.feeder)
            .await?
            .status
            .map(|(status, _)| status.boot_count);
        // Where the feed's result can start: after the newest event held,
        // or from the start of the live session's history when none of it
        // is held yet (a session's replay may still be arriving).
        let held = self
            .shared
            .store
            .feeder(write.feeder)
            .await?
            .and_then(|row| row.synced);
        let history = match self.mirror.epoch(write.feeder) {
            Some(live) if held.is_none_or(|held| held.epoch != live) => Some(Synced {
                epoch: live,
                seq: 0,
            }),
            _ => held,
        };

        let entry = NewCommand {
            key,
            user: write.caller.user.id,
            token: write.caller.token,
            feeder: write.feeder,
            kind: write.kind,
            body_hash,
            request_id: request,
            boot_count,
            history,
            sent: self.shared.clock.now(),
        };
        let (request, attempt) = match self.shared.store.journal(&entry).await? {
            Journaled::New => (request, Attempt::First),
            Journaled::Existing(row) => {
                let same = row.user == entry.user
                    && row.feeder == entry.feeder
                    && row.kind == entry.kind
                    && row.body_hash == entry.body_hash;
                if !same {
                    return Err(CommandError::KeyReused.into());
                }
                match retry(&row) {
                    Retry::Replay(outcome) => {
                        return Ok(Recorded {
                            key,
                            outcome,
                            invite: None,
                        });
                    }
                    Retry::Unknown => return Err(CommandError::Unknown.into()),
                    Retry::Resend(request) => (request, Attempt::Retry),
                }
            }
        };

        let service = self.clone();
        let feeder = write.feeder;
        let op = write.op;
        let sending = self
            .tasks
            .spawn(async move { service.send(key, feeder, request, op, attempt).await });
        let Ok(sent) = sending.await else {
            // The task died with the write possibly sent and nothing
            // recorded: say unknown now rather than leave it `sent`, which
            // a retry would take for a send still in flight.
            self.unknown_best_effort(&key).await;
            return Err(CommandError::Unknown.into());
        };
        let (outcome, invite) = sent?;
        Ok(Recorded {
            key,
            outcome,
            invite,
        })
    }

    /// Mark `key`'s entry unknown after a failure that left it `sent`,
    /// logging rather than returning a failure of its own: the caller
    /// already has the error that matters.
    async fn unknown_best_effort(&self, key: &IdempotencyKey) {
        let now = self.shared.clock.now();
        if let Err(error) = self.unknown(key, now).await {
            warn!(key = %Uuid::from_bytes(*key), %error, "a write's entry stays sent until the next start");
        }
    }

    /// Replace a recorded outcome: the settings read back after a change.
    pub(crate) async fn amend(&self, key: &IdempotencyKey, outcome: &CommandOutcome) -> Result<()> {
        let now = self.shared.clock.now();
        if !self.shared.store.answer_command(key, outcome, now).await? {
            warn!(key = %Uuid::from_bytes(*key), "no journal entry took the settings read back");
        }
        Ok(())
    }

    /// Mark the writes a stopped server left `sent` unknown.
    pub(crate) async fn interrupt_commands(&self) -> Result<()> {
        let now = self.shared.clock.now();
        let interrupted = self.shared.store.interrupt_commands(now).await?;
        if interrupted > 0 {
            warn!("{interrupted} writes sent before the last stop are unknown");
        }
        Ok(())
    }

    /// Send one attempt and record what came of it: the outcome, and an
    /// invite's secret beside it.
    async fn send(
        &self,
        key: IdempotencyKey,
        feeder: FeederId,
        request: RequestId,
        op: Op,
        attempt: Attempt,
    ) -> Result<(CommandOutcome, Option<Invite>)> {
        let answer = self.mirror.ask(feeder, request, op).await;
        let store = &self.shared.store;
        let now = self.shared.clock.now();

        let mut invite = None;
        let outcome = match answer {
            Ok(Answer::Done { .. }) => CommandOutcome::Done,
            Ok(Answer::Invite(made)) => {
                invite = Some(made);
                CommandOutcome::Invited {
                    invite: made.id,
                    seconds: made.seconds,
                }
            }
            Ok(Answer::Refused(refusal)) => CommandOutcome::Refused {
                refusal: refusal.into(),
            },
            Ok(Answer::Feeding { feed }) => CommandOutcome::Feeding { feed },
            // An answer that fits no write: what the feeder did is not
            // known from it.
            Ok(_) => {
                self.unknown(&key, now).await?;
                return Err(InternalError::Unexpected.into());
            }
            Err(AskError::Unknown) => {
                self.unknown(&key, now).await?;
                return Err(CommandError::Unknown.into());
            }
            // Never sent: a first attempt leaves no entry, so the key is
            // free for the same write later. Only the first attempt sends
            // while the entry is `sent`, so no other attempt is using it.
            Err(unsent @ (AskError::NotRunning | AskError::Offline | AskError::Unsendable)) => {
                if matches!(attempt, Attempt::First) {
                    store.forget_unsent(&key).await?;
                }
                return Err(Error::from(unsent));
            }
        };
        match store.answer_command(&key, &outcome, now).await {
            Ok(true) => self.published(&key).await,
            Ok(false) => {
                warn!(key = %Uuid::from_bytes(key), "the feeder answered a write the journal no longer holds open");
            }
            // The answer is lost to the journal; unknown is still true of
            // it, and a retry resends under the same id.
            Err(error) => {
                self.unknown_best_effort(&key).await;
                return Err(error.into());
            }
        }
        // The feed's `Fed` event may have been stored before its answer
        // was: look for it now rather than at the next event, beside the
        // answer rather than ahead of it.
        if outcome.feed().is_some() {
            let service = self.clone();
            self.tasks
                .spawn(async move { service.reconcile_best_effort(feeder).await });
        }
        Ok((outcome, invite))
    }

    /// Tell the stream that `key`'s entry changed, as it now stands. A
    /// read that fails is logged: the entry itself is recorded.
    pub(crate) async fn published(&self, key: &IdempotencyKey) {
        match self.shared.store.command(key).await {
            Ok(Some(row)) => self
                .shared
                .hub
                .publish(StreamEvent::Command { command: view(row) }),
            Ok(None) => {}
            Err(error) => {
                warn!(key = %Uuid::from_bytes(*key), %error, "a changed entry was not streamed");
            }
        }
    }

    /// Mark `key`'s entry unknown. An entry another attempt answered stays
    /// answered; one that is gone is logged.
    async fn unknown(&self, key: &IdempotencyKey, now: jiff::Timestamp) -> Result<()> {
        if self.shared.store.command_unknown(key, now).await? {
            self.published(key).await;
        } else {
            let state = self.shared.store.command(key).await?.map(|row| row.state);
            if state != Some(CommandState::Answered) {
                warn!(key = %Uuid::from_bytes(*key), ?state, "an unanswered write's entry was not open");
            }
        }
        Ok(())
    }
}

pub(crate) fn view(row: CommandRow) -> Command {
    Command {
        key: Uuid::from_bytes(row.key),
        user: row.user,
        token: row.token,
        feeder: row.feeder,
        kind: row.kind,
        state: row.state,
        outcome: row.outcome,
        feed: row.feed,
        boot_count: row.boot_count,
        sent: row.sent,
        updated: row.updated,
    }
}
