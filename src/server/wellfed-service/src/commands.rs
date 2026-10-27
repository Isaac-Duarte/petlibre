//! Commands sent to a feeder: its live status, its settings and the panel
//! lock. Each goes through the mirror under a device request id; a write's
//! id is bound to who sent it (`write_id`), so a retry under the
//! same key is answered from the feeder's memory and never applied twice.

use wellfed_api::{
    FeederDetail, FeederId, Generation, Settings, SettingsPatch, StatusSnapshot, User,
};
use wellfed_model::{Answer, Op, Refusal};
use sha2::{Digest, Sha256};

use crate::{CommandError, Error, IdempotencyKey, Invalid, RequestId, Result, Service};

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
    /// the settings are read again; if that read fails the change still
    /// happened, and a retry under the same `request` id is answered from
    /// the feeder's memory without changing anything twice.
    pub async fn put_settings(
        &self,
        user: &User,
        id: FeederId,
        base: Generation,
        patch: SettingsPatch,
        key: Option<IdempotencyKey>,
    ) -> Result<Settings> {
        if patch.volume.is_some_and(|volume| volume > 100) {
            return Err(Invalid::Volume.into());
        }
        self.row(id).await?;

        let op = Op::PutSettings {
            base,
            changes: patch,
        };
        let request = self.write_id(user, id, Write::Settings, key)?;
        match self.ask(id, request, op).await? {
            // The change is made; if the read-back fails, saying offline
            // would claim it was not, so the outcome is unknown: a retry
            // under the same key is answered without changing anything.
            Answer::Done { .. } => self.read_settings(id).await.map_err(|error| match error {
                Error::Command(CommandError::Offline) => CommandError::Unknown.into(),
                other => other,
            }),
            Answer::Refused(Refusal::Stale { .. }) => {
                Err(CommandError::Stale(self.read_settings(id).await?).into())
            }
            other => Err(refused(&other)),
        }
    }

    pub async fn set_lock(
        &self,
        user: &User,
        id: FeederId,
        locked: bool,
        key: Option<IdempotencyKey>,
    ) -> Result<()> {
        self.row(id).await?;
        let request = self.write_id(user, id, Write::Lock, key)?;
        match self.ask(id, request, Op::SetLock { locked }).await? {
            Answer::Done { .. } => Ok(()),
            other => Err(refused(&other)),
        }
    }

    /// The device request id for a write. The feeder remembers writes by id
    /// alone, and every user shares the server's key, so the client's key
    /// is bound to who sent it, to which feeder and to what kind of write:
    /// one key reused for a lock and then a settings change cannot return
    /// the lock's answer, and two users' keys never meet. A write without a
    /// key gets a fresh id.
    fn write_id(
        &self,
        user: &User,
        id: FeederId,
        write: Write,
        key: Option<IdempotencyKey>,
    ) -> Result<RequestId> {
        let Some(key) = key else {
            return self.request_id();
        };
        let digest = Sha256::new()
            .chain_update(b"wellfed-server write v1")
            .chain_update(user.id.0.as_bytes())
            .chain_update(id.0.as_bytes())
            .chain_update([write.code()])
            .chain_update(key)
            .finalize();
        let mut request = [0; 16];
        request.copy_from_slice(&digest[..16]);
        Ok(request)
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

    /// Send `op` to the feeder through the mirror and wait for its answer.
    async fn ask(&self, id: FeederId, request: RequestId, op: Op) -> Result<Answer> {
        Ok(self.mirror.ask(id, request, op).await?)
    }
}

/// A write's kind, bound into its request id. The codes are hashed, so they
/// never change.
#[derive(Clone, Copy)]
enum Write {
    Settings,
    Lock,
}

impl Write {
    const fn code(self) -> u8 {
        match self {
            Self::Settings => 1,
            Self::Lock => 2,
        }
    }
}

/// The error for an answer that is not the one asked for.
fn refused(answer: &Answer) -> Error {
    let error = match answer {
        Answer::Refused(refusal) => CommandError::Refused((*refusal).into()),
        _ => CommandError::Unexpected,
    };
    error.into()
}
