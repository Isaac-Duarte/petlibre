//! Adopting feeders, and reading them and their history as the server
//! holds it.

use wellfed_api::{AdoptFeeder, Connection as Link, EventPage, EventQuery, Feeder, FeederId, Hex};
use wellfed_formats::hex;
use wellfed_mirror::Invite;
use wellfed_store::{FeederRow, NewFeeder};
use tracing::warn;

use crate::{Error, FeederError, Invalid, Result, Service, keys};

const PAGE: u32 = 100;
const PAGE_MAX: u32 = 500;
/// Past every stored sequence number (SQLite integers are `i64`).
const NEWEST: u64 = i64::MAX.unsigned_abs();

impl Service {
    /// Adopt a feeder: redeem the invite with a new key of the server's
    /// own, store the key sealed, and start mirroring the feeder.
    ///
    /// Adoptions run one at a time: two racing for one feeder would both
    /// enroll a key, and the loser's would hold a client slot on the feeder
    /// with nothing on the server to use it.
    pub async fn adopt(&self, request: AdoptFeeder) -> Result<Feeder> {
        let name = feeder_name(&request.name)?;
        let invite = Invite::parse(&request.invite).map_err(FeederError::from)?;
        let _one_at_a_time = self.adopting.lock().await;
        if self.shared.store.feeder_adopted(&invite.feeder).await? {
            return Err(FeederError::AlreadyAdopted.into());
        }
        let identity = invite.feeder;
        let address = self
            .mirror
            .locate(identity, request.address.as_deref())
            .await?;

        let secret = self.shared.random::<32>()?;
        let label = &self.shared.config.label;
        let enrolled = self.mirror.enroll(invite, label, address, secret).await?;

        let id = FeederId(self.shared.new_id()?);
        let nonce = self.shared.random()?;
        let sealed = keys::seal(&self.shared.config.key, id, &identity, &secret, nonce)?;
        let address = address.to_string();
        let inserted = self
            .shared
            .store
            .insert_feeder(NewFeeder {
                id,
                identity,
                name,
                role: enrolled.role,
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
                hex::encode(&identity[..4]),
                hex::encode(&enrolled.public[..4]),
            );
            return Err(match error {
                wellfed_store::Error::Duplicate(_) => FeederError::AlreadyAdopted.into(),
                other => other.into(),
            });
        }

        let row = self.row(id).await?;
        self.spawn(&row).await?;
        Ok(self.view(row))
    }

    pub async fn feeders(&self) -> Result<Vec<Feeder>> {
        let rows = self.shared.store.feeders().await?;
        Ok(rows.into_iter().map(|row| self.view(row)).collect())
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
            .iter()
            .map(|row| {
                wellfed_mirror::stored_event(epoch, row)
                    .ok_or(Error::Feeder(FeederError::Undecodable(row.seq)))
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

    pub(crate) async fn row(&self, id: FeederId) -> Result<FeederRow> {
        let row = self.shared.store.feeder(id).await?;
        row.ok_or_else(|| FeederError::NotFound.into())
    }

    pub(crate) fn view(&self, row: FeederRow) -> Feeder {
        let connection = self.links().get(&row.id).copied().unwrap_or(Link::Offline);
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
}

fn feeder_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Invalid::FeederName.into());
    }
    Ok(name)
}
