//! A feeder's clients as the server sees them: its own key, the phones it
//! invited (whose each is), the clients an owner kept, and the unowned
//! rest. Listing and revoking are an owner's; the feeder lets only an
//! owner's key revoke, so a server adopted with the `server` role is
//! refused `forbidden` by the feeder.

use wellfed_api::{
    ClientOwner, CommandKind, CommandOutcome, FeederClient, FeederId, FeederRefusal, Hex,
};
use wellfed_model::{Answer, Client, Op};
use wellfed_store::FeederRow;
use serde::Serialize;

use crate::commands::{done, refused};
use crate::journal::Write;
use crate::{Caller, ClientError, IdempotencyKey, Result, Service, keys};

/// A revoke as it is hashed into the journal.
#[derive(Serialize)]
struct RevokeClient {
    key: Hex<32>,
}

impl Service {
    /// The feeder's clients, read from it, each with whose it is.
    pub async fn clients(&self, id: FeederId) -> Result<Vec<FeederClient>> {
        self.live_row(id).await?;
        let listed = self.list_clients(id).await?;
        let phones = self.shared.store.feeder_phones(id).await?;
        let kept = self.shared.store.kept_clients(id).await?;

        let owner = |client: &Client| {
            if client.you {
                return ClientOwner::Server;
            }
            if let Some(phone) = phones.iter().find(|phone| phone.key == client.key) {
                return ClientOwner::Phone {
                    user: phone.user,
                    token: phone.token,
                    revoking: phone.revoking.clone(),
                };
            }
            if kept.contains(&client.key.0) {
                ClientOwner::Kept
            } else {
                ClientOwner::Unowned
            }
        };
        Ok(listed
            .iter()
            .map(|client| FeederClient {
                key: client.key,
                label: client.label.clone(),
                role: client.role,
                owner: owner(client),
            })
            .collect())
    }

    /// Keep a client the server did not invite, so it no longer shows as
    /// unowned. Nothing is sent to the feeder.
    pub async fn keep_client(&self, id: FeederId, key: [u8; 32]) -> Result<()> {
        self.live_row(id).await?;
        let now = self.shared.clock.now();
        Ok(self.shared.store.keep_client(id, &key, now).await?)
    }

    /// Revoke one of the feeder's clients through the journal; one the
    /// feeder no longer holds counts as revoked. The server's own key is
    /// refused: removing the feeder revokes it.
    pub async fn revoke_client(
        &self,
        caller: &Caller,
        id: FeederId,
        key: [u8; 32],
        idempotency: Option<IdempotencyKey>,
    ) -> Result<()> {
        let row = self.live_row(id).await?;
        if key == self.own_key(&row)? {
            return Err(ClientError::OwnKey.into());
        }

        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Revoke,
                body: &RevokeClient { key: Hex(key) },
                key: idempotency,
                op: Op::RevokeClient { key },
            })
            .await?;
        // A client already gone is revoked: a retry after the first
        // landed, or a revoke from elsewhere.
        let gone = CommandOutcome::Refused {
            refusal: FeederRefusal::NotFound,
        };
        if recorded.outcome != gone {
            done(&recorded.outcome)?;
        }
        self.shared.store.forget_client(id, &key).await?;
        Ok(())
    }

    /// The feeder's client table, read now.
    pub(crate) async fn list_clients(&self, id: FeederId) -> Result<Vec<Client>> {
        match self.ask(id, self.request_id()?, Op::ListClients).await? {
            Answer::Clients(clients) => Ok(clients),
            other => Err(refused(&other)),
        }
    }

    /// The server's public key on the feeder.
    pub(crate) fn own_key(&self, row: &FeederRow) -> Result<[u8; 32]> {
        let secret = keys::open(&self.shared.config.key, row.id, &row.identity, &row.key)?;
        let feeder = wellfed_mirror::Feeder {
            id: row.id,
            identity: row.identity,
            secret,
            address: None,
        };
        Ok(feeder.public())
    }
}
