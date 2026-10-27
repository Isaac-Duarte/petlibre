//! The command journal: every write the server sends a feeder is recorded
//! before it is sent, under its idempotency key, and its answer after.
//!
//! The feeder dedups writes by device request id, but only the last 32
//! per client, and every user shares the server's key: a retry that
//! arrives after 32 other writes would run again. So a retry reads the
//! journal first. An answered write returns its recorded outcome and never
//! reaches the feeder; one whose outcome is unknown goes out again under
//! the request id it was first sent under, which the feeder answers from
//! memory if it ran it. A key that already names another request is
//! refused and nothing is sent.
//!
//! The send runs in a task of its own, so the entry reaches its answer or
//! `unknown` even when the caller stops waiting. A server that stops
//! before then leaves the entry `sent`, and the next start marks it
//! `unknown`: the feeder may have run it, so it is never called failed.

use wellfed_api::{
    Command, CommandKind, CommandOutcome, CommandPage, CommandQuery, CommandState, FeederId, Role,
    User,
};
use wellfed_formats::canonical::canonical_of;
use wellfed_mirror::AskError;
use wellfed_model::{Answer, Op};
use wellfed_store::{CommandFilter, CommandRow, Journaled, NewCommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{CommandError, Error, IdempotencyKey, Invalid, RequestId, Result, Service};

const PAGE: u32 = 100;
const PAGE_MAX: u32 = 1000;

/// A write to send through the journal.
pub(crate) struct Write<'a, B> {
    pub(crate) user: &'a User,
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
            Some(row) if user.role == Role::Owner || row.user == user.id => Ok(view(row)),
            _ => Err(CommandError::NoEntry.into()),
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
            .map(|after| after.parse::<u64>().map_err(|_| Invalid::CommandCursor))
            .transpose()?;

        // One more than the page shows whether another page follows.
        let mut rows = self
            .shared
            .store
            .commands(CommandFilter {
                user: (user.role != Role::Owner).then_some(user.id),
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
        let body = canonical_of(write.body).map_err(CommandError::Body)?;
        let body_hash: [u8; 32] = Sha256::digest(body).into();
        let request = write_id(write.user, write.feeder, write.kind, &key);
        let boot_count = self
            .shared
            .store
            .state(write.feeder)
            .await?
            .status
            .map(|(status, _)| status.boot_count);

        let entry = NewCommand {
            key,
            user: write.user.id,
            feeder: write.feeder,
            kind: write.kind,
            body_hash,
            request_id: request,
            boot_count,
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
                match row.state {
                    CommandState::Answered | CommandState::Resolved => {
                        let outcome = row.outcome.ok_or(CommandError::Unknown)?;
                        return Ok(Recorded { key, outcome });
                    }
                    CommandState::Sent | CommandState::Unknown => (row.request_id, Attempt::Retry),
                }
            }
        };

        let service = self.clone();
        let feeder = write.feeder;
        let op = write.op;
        let sending = self
            .tasks
            .spawn(async move { service.send(key, feeder, request, op, attempt).await });
        let outcome = sending
            .await
            .unwrap_or_else(|_| Err(CommandError::Unknown.into()))?;
        Ok(Recorded { key, outcome })
    }

    /// Replace a recorded outcome: the settings read back after a change.
    pub(crate) async fn amend(&self, key: &IdempotencyKey, outcome: &CommandOutcome) -> Result<()> {
        let now = self.shared.clock.now();
        Ok(self.shared.store.answer_command(key, outcome, now).await?)
    }

    /// Mark the writes a stopped server left `sent` unknown.
    pub(crate) async fn interrupt_commands(&self) -> Result<()> {
        let now = self.shared.clock.now();
        let interrupted = self.shared.store.interrupt_commands(now).await?;
        if interrupted > 0 {
            tracing::warn!("{interrupted} writes sent before the last stop are unknown");
        }
        Ok(())
    }

    /// Send one attempt and record what came of it.
    async fn send(
        &self,
        key: IdempotencyKey,
        feeder: FeederId,
        request: RequestId,
        op: Op,
        attempt: Attempt,
    ) -> Result<CommandOutcome> {
        let answer = self.mirror.ask(feeder, request, op).await;
        let store = &self.shared.store;
        let now = self.shared.clock.now();

        let outcome = match answer {
            Ok(Answer::Done { .. }) => CommandOutcome::Done,
            Ok(Answer::Refused(refusal)) => CommandOutcome::Refused {
                refusal: refusal.into(),
            },
            // An answer that fits no write: what the feeder did is not
            // known from it.
            Ok(_) => {
                store.command_unknown(&key, now).await?;
                return Err(CommandError::Unexpected.into());
            }
            Err(AskError::Unknown) => {
                store.command_unknown(&key, now).await?;
                return Err(CommandError::Unknown.into());
            }
            // Never sent: a first attempt leaves no entry, so the key is
            // free for the same write later.
            Err(unsent @ (AskError::NotRunning | AskError::Offline | AskError::Unsendable)) => {
                if matches!(attempt, Attempt::First) {
                    store.forget_unsent(&key).await?;
                }
                return Err(Error::from(unsent));
            }
        };
        store.answer_command(&key, &outcome, now).await?;
        Ok(outcome)
    }
}

/// The device request id for a write. The feeder remembers writes by id
/// alone and every user shares the server's key, so the id is bound to who
/// sent it, to which feeder, to what kind of write and to its key.
fn write_id(user: &User, feeder: FeederId, kind: CommandKind, key: &IdempotencyKey) -> RequestId {
    let digest = Sha256::new()
        .chain_update(b"wellfed-server write v1")
        .chain_update(user.id.0.as_bytes())
        .chain_update(feeder.0.as_bytes())
        .chain_update([kind_code(kind)])
        .chain_update(key)
        .finalize();
    let mut request = [0; 16];
    request.copy_from_slice(&digest[..16]);
    request
}

/// A write's kind as it is hashed into its request id: these never change.
const fn kind_code(kind: CommandKind) -> u8 {
    match kind {
        CommandKind::Settings => 1,
        CommandKind::Lock => 2,
    }
}

fn view(row: CommandRow) -> Command {
    Command {
        key: Uuid::from_bytes(row.key),
        user: row.user,
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
