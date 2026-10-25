//! Adopting feeders, reading them and changing them through their tasks.

use std::net::SocketAddr;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use wellfed_api::{
    AdoptFeeder, Connection as Link, EventPage, EventQuery, Feeder, FeederDetail, FeederEvent,
    FeederId, Generation, Hex, Settings, SettingsPatch, StatusSnapshot, User,
};
use wellfed_client::api::Invite;
use wellfed_model::{Answer, Event, EventBody, Joined, Op, Refusal};
use wellfed_proto::api as proto;
use wellfed_session::hex::Lower;
use wellfed_session::noise::Keypair;
use wellfed_store::{FeederRow, NewFeeder};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio::time::{Instant, timeout_at};
use tracing::warn;
use uuid::Uuid;

use crate::feeder::{self, Command, Handle};
use crate::{Discovery, Error, IdempotencyKey, Invalid, RequestId, Result, Service, keys};

const PAGE: u32 = 100;
const PAGE_MAX: u32 = 500;
/// Past every stored sequence number (SQLite integers are `i64`).
const NEWEST: u64 = i64::MAX.unsigned_abs();

impl Service {
    pub(crate) fn spawn(&self, row: &FeederRow) -> Result<()> {
        let handle = feeder::spawn(
            Arc::clone(&self.shared),
            row,
            self.cancel.child_token(),
            &self.tasks,
        )?;
        self.handles().insert(row.id, handle);
        Ok(())
    }

    /// Adopt a feeder: redeem the invite with a new key of the server's
    /// own, store the key sealed, and start the feeder's task.
    ///
    /// Adoptions run one at a time: two racing for one feeder would both
    /// enroll a key, and the loser's would hold a client slot on the feeder
    /// with nothing on the server to use it.
    pub async fn adopt(&self, request: AdoptFeeder) -> Result<Feeder> {
        let name = feeder_name(&request.name)?;
        let invite = Invite::parse(&request.invite)?;
        let _one_at_a_time = self.adopting.lock().await;
        if self.shared.store.feeder_adopted(&invite.feeder).await? {
            return Err(Error::AlreadyAdopted);
        }
        let address = self
            .resolve(invite.feeder, request.address.as_deref())
            .await?;

        let secret = self.shared.random::<32>()?;
        let keys = Keypair::from_secret(secret);
        let label = &self.shared.config.label;
        let joined = wellfed_connect::join(&keys, invite, label, address, self.shared.random()?)
            .await
            .map_err(|error| match error {
                // The feeder drops an unknown or expired invite unanswered.
                wellfed_connect::Error::Closed => Error::InviteExpired,
                other => Error::Unreachable(other),
            })?;
        let role = match joined {
            Joined::Enrolled { role } => role,
            Joined::Refused(refusal) => return Err(Error::Refused(refusal.into())),
        };

        let id = FeederId(Uuid::now_v7());
        let nonce = self.shared.random()?;
        let sealed = keys::seal(&self.shared.config.key, id, &invite.feeder, &secret, nonce)?;
        let address = address.to_string();
        let inserted = self
            .shared
            .store
            .insert_feeder(NewFeeder {
                id,
                identity: invite.feeder,
                name,
                role,
                address: Some(&address),
                key: sealed,
                created: self.shared.clock.now(),
            })
            .await;
        if let Err(error) = inserted {
            // The feeder holds the key now, but the server cannot use it.
            warn!(
                "feeder {} enrolled this server's key ({}) but it was not stored: {error}; \
                 revoke that client on the feeder",
                Lower(&invite.feeder[..4]),
                Lower(&keys.public()[..4]),
            );
            return Err(match error {
                wellfed_store::Error::Duplicate(_) => Error::AlreadyAdopted,
                other => other.into(),
            });
        }

        let row = self.row(id).await?;
        self.spawn(&row)?;
        Ok(self.view(row))
    }

    pub async fn feeders(&self) -> Result<Vec<Feeder>> {
        let rows = self.shared.store.feeders().await?;
        Ok(rows.into_iter().map(|row| self.view(row)).collect())
    }

    /// The feeder and its status: read now when it is online, otherwise the
    /// last copy the server holds.
    pub async fn feeder(&self, id: FeederId) -> Result<FeederDetail> {
        let row = self.row(id).await?;
        let live = match self.ask(id, self.request_id()?, Op::Status).await {
            Ok(answer) => match answer {
                Answer::Status(status) => Some(status),
                other => return Err(refused(&other)),
            },
            Err(Error::Offline | Error::Unknown) => None,
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
    /// [`Error::Stale`] with the current settings. After the change the
    /// settings are read again; if that read fails the change still
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
                Error::Offline => Error::Unknown,
                other => other,
            }),
            Answer::Refused(Refusal::Stale { .. }) => {
                Err(Error::Stale(self.read_settings(id).await?))
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

    /// A page of the feeder's history as the server holds it.
    pub async fn events(&self, id: FeederId, query: EventQuery) -> Result<EventPage> {
        let limit = query.limit.unwrap_or(PAGE);
        if !(1..=PAGE_MAX).contains(&limit) {
            return Err(Invalid::Limit.into());
        }
        if query.after.is_some_and(|after| after > NEWEST) {
            return Err(Invalid::Cursor.into());
        }
        let row = self.row(id).await?;
        let Some(epoch) = query.epoch.or(row.synced.map(|synced| synced.epoch)) else {
            return Ok(EventPage {
                epoch: None,
                events: Vec::new(),
                gaps: Vec::new(),
                next: None,
            });
        };

        let rows = self
            .shared
            .store
            .events(id, epoch, query.after, limit, query.kind)
            .await?;
        let full = u32::try_from(rows.len()).is_ok_and(|len| len == limit);
        let events = rows
            .into_iter()
            .map(|row| {
                let body: proto::EventBody =
                    wellfed_proto::decode(&row.body).map_err(|_| Error::Undecodable(row.seq))?;
                let event = Event {
                    epoch,
                    seq: row.seq,
                    at: row.at,
                    body: EventBody::from(body),
                };
                Ok(FeederEvent::new(event, row.received))
            })
            .collect::<Result<Vec<_>>>()?;

        let next = events.last().map(|event| event.seq).filter(|_| full);
        let gaps = self
            .shared
            .store
            .gaps(id, epoch, query.after, next.unwrap_or(NEWEST))
            .await?;
        Ok(EventPage {
            epoch: Some(epoch),
            events,
            gaps,
            next,
        })
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

    /// Hand `op` to the feeder's task and wait for its answer. Turned away
    /// at once while the feeder is offline; [`Error::Unknown`] when the
    /// answer does not come in time.
    async fn ask(&self, id: FeederId, request: RequestId, op: Op) -> Result<Answer> {
        let (commands, link) = {
            let handles = self.handles();
            let handle = handles.get(&id).ok_or(Error::NotFound)?;
            (handle.commands.clone(), handle.connection())
        };
        if link != Link::Online {
            return Err(Error::Offline);
        }

        let deadline = Instant::now() + self.shared.config.timing.answer;
        let (reply, answer) = oneshot::channel();
        let command = Command {
            id: request,
            op,
            reply,
        };
        match timeout_at(deadline, commands.send(command)).await {
            Ok(Ok(())) => {}
            // Never handed over, so never sent.
            Ok(Err(_)) | Err(_) => return Err(Error::Offline),
        }
        match timeout_at(deadline, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) | Err(_) => Err(Error::Unknown),
        }
    }

    async fn row(&self, id: FeederId) -> Result<FeederRow> {
        self.shared.store.feeder(id).await?.ok_or(Error::NotFound)
    }

    fn view(&self, row: FeederRow) -> Feeder {
        let connection = self
            .handles()
            .get(&row.id)
            .map_or(Link::Offline, Handle::connection);
        Feeder {
            id: row.id,
            name: row.name,
            identity: Hex(row.identity),
            role: row.role,
            address: row.address,
            connection,
            synced: row.synced.map(|synced| synced.seq),
            epoch: row.synced.map(|synced| synced.epoch),
        }
    }

    fn handles(&self) -> MutexGuard<'_, std::collections::HashMap<FeederId, Handle>> {
        // Insertions are whole, so a poisoned map is still consistent.
        self.feeders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Where to redeem an invite: the given address, or the feeder found
    /// over mDNS.
    async fn resolve(&self, identity: [u8; 32], address: Option<&str>) -> Result<SocketAddr> {
        match address.map(str::trim).filter(|address| !address.is_empty()) {
            Some(address) => wellfed_connect::locate(&identity, Some(address), Duration::ZERO)
                .map_err(|_| Invalid::Address.into()),
            None if self.shared.config.discovery == Discovery::Off => {
                Err(Invalid::AddressRequired.into())
            }
            None => feeder::discover(identity).await,
        }
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

fn feeder_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Invalid::FeederName.into());
    }
    Ok(name)
}

/// The error for an answer that is not the one asked for.
fn refused(answer: &Answer) -> Error {
    match answer {
        Answer::Refused(refusal) => Error::Refused((*refusal).into()),
        _ => Error::Unexpected,
    }
}
