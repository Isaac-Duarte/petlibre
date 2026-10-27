//! Commands sent to a feeder: its live status, its settings and the panel
//! lock. Reads go straight through the mirror under a fresh request id;
//! every write goes through the journal (`journal.rs`), so a retry under
//! the same key is answered once and never applied twice.

use wellfed_api::{
    CommandKind, CommandOutcome, FeederDetail, FeederId, FeederRefusal, Generation, LockRequest,
    Settings, SettingsPatch, StatusSnapshot,
};
use wellfed_model::{Answer, Op};
use serde::Serialize;

use crate::journal::Write;
use crate::{Caller, CommandError, Error, IdempotencyKey, Invalid, RequestId, Result, Service};

/// A settings change as it is hashed into the journal: the base it names
/// is part of the request.
#[derive(Serialize)]
struct SettingsChange {
    base: Generation,
    changes: SettingsPatch,
}

impl Service {
    /// The feeder and its status: read now when it is online, otherwise the
    /// last copy the server holds.
    pub async fn feeder(&self, id: FeederId) -> Result<FeederDetail> {
        let row = self.row(id).await?;
        let live = match self.ask(id, self.request_id()?, Op::Status).await {
            Ok(answer) => match answer {
                Answer::Status(status) => Some(status),
                other => return Err(refused(&other)),
            },
            Err(Error::Command(CommandError::Offline | CommandError::Unknown)) => None,
            Err(error) => return Err(error),
        };

        let status = match live {
            Some(status) => {
                let at = self.shared.clock.now();
                self.shared.store.record_status(id, &status, at).await?;
                Some(StatusSnapshot {
                    at,
                    live: true,
                    status,
                })
            }
            None => self
                .shared
                .store
                .state(id)
                .await?
                .status
                .map(|(status, at)| StatusSnapshot {
                    at,
                    live: false,
                    status,
                }),
        };
        Ok(FeederDetail {
            feeder: self.view(row),
            status,
        })
    }

    /// The settings, read from the feeder: a change must name the current
    /// generation, so a copy would only invite a stale write.
    pub async fn settings(&self, id: FeederId) -> Result<Settings> {
        self.row(id).await?;
        self.read_settings(id).await
    }

    /// Change the settings if they are still at `base`. A stale base gets
    /// [`CommandError::Stale`] with the current settings. After the change
    /// the settings are read again and recorded as the write's outcome, so
    /// a retry under the same key returns them; if that read fails the
    /// change still happened, and a retry reads them again.
    pub async fn put_settings(
        &self,
        caller: &Caller,
        id: FeederId,
        base: Generation,
        patch: SettingsPatch,
        key: Option<IdempotencyKey>,
    ) -> Result<Settings> {
        if patch.volume.is_some_and(|volume| volume > 100) {
            return Err(Invalid::Volume.into());
        }
        self.live_row(id).await?;

        let body = SettingsChange {
            base,
            changes: patch,
        };
        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Settings,
                body: &body,
                key,
                op: Op::PutSettings {
                    base,
                    changes: patch,
                },
            })
            .await?;

        match recorded.outcome {
            CommandOutcome::Settings { settings } => Ok(settings),
            // The change is made; if the read-back fails, saying offline
            // would claim it was not, so the outcome is unknown: a retry
            // under the same key reads it again without changing anything.
            CommandOutcome::Done => {
                let settings = self.read_settings(id).await.map_err(|error| match error {
                    Error::Command(CommandError::Offline) => CommandError::Unknown.into(),
                    other => other,
                })?;
                self.amend(&recorded.key, &CommandOutcome::Settings { settings })
                    .await?;
                Ok(settings)
            }
            // A stale refusal's answer is the current settings, read now,
            // so its replay needs the feeder online: offline it is
            // `Offline`, which is true of the write too (it was refused and
            // nothing ran), and a later retry replays it again. A cached
            // copy would offer a base that may be stale itself.
            CommandOutcome::Refused {
                refusal: FeederRefusal::Stale,
            } => Err(CommandError::Stale(self.read_settings(id).await?).into()),
            CommandOutcome::Refused { refusal } => Err(CommandError::Refused(refusal).into()),
            CommandOutcome::Feeding { .. }
            | CommandOutcome::Fed { .. }
            | CommandOutcome::Invited { .. }
            | CommandOutcome::Unrecorded => Err(CommandError::Unexpected.into()),
        }
    }

    pub async fn set_lock(
        &self,
        caller: &Caller,
        id: FeederId,
        locked: bool,
        key: Option<IdempotencyKey>,
    ) -> Result<()> {
        self.live_row(id).await?;

        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Lock,
                body: &LockRequest { locked },
                key,
                op: Op::SetLock { locked },
            })
            .await?;
        done(&recorded.outcome)
    }

    async fn read_settings(&self, id: FeederId) -> Result<Settings> {
        let settings = match self.ask(id, self.request_id()?, Op::GetSettings).await? {
            Answer::Settings(settings) => settings,
            other => return Err(refused(&other)),
        };
        let now = self.shared.clock.now();
        self.shared
            .store
            .record_settings(id, &settings, now)
            .await?;
        Ok(settings)
    }

    /// Send a read to the feeder through the mirror and wait for its
    /// answer.
    pub(crate) async fn ask(&self, id: FeederId, request: RequestId, op: Op) -> Result<Answer> {
        Ok(self.mirror.ask(id, request, op).await?)
    }
}

/// A write that takes effect or is refused, and has no answer beyond.
pub(crate) fn done(outcome: &CommandOutcome) -> Result<()> {
    match outcome {
        CommandOutcome::Done => Ok(()),
        CommandOutcome::Refused { refusal } => Err(CommandError::Refused(*refusal).into()),
        CommandOutcome::Settings { .. }
        | CommandOutcome::Feeding { .. }
        | CommandOutcome::Fed { .. }
        | CommandOutcome::Invited { .. }
        | CommandOutcome::Unrecorded => Err(CommandError::Unexpected.into()),
    }
}

/// The error for an answer that is not the one asked for.
pub(crate) fn refused(answer: &Answer) -> Error {
    let error = match answer {
        Answer::Refused(refusal) => CommandError::Refused((*refusal).into()),
        _ => CommandError::Unexpected,
    };
    error.into()
}
