//! Phones' own keys on the feeders, and revoking them.
//!
//! An owner's or member's phone keeps a key on every feeder, for the LAN
//! while the server is down. The server makes the invite it joins with,
//! recording the key, account and token first, so a phone that joins is
//! always attributed even when the invite's answer is lost. A viewer, and
//! an account or token with an expiry, holds no key: the server alone
//! enforces its limits.
//!
//! Deleting, demoting or putting an expiry on an account, or revoking a
//! token, marks its keys for revocation on every feeder. Each feeder
//! revokes them at once when online, and again each time it reconnects,
//! until it has dropped them; until then each key shows the last attempt,
//! so an owner sees which feeders still hold one. A revoke is idempotent
//! (a key already gone is refused `not_found`, which settles it), so each
//! attempt takes a fresh request id and needs no journal entry.

use std::sync::Arc;

use wellfed_api::{
    ClientRole, CommandKind, CommandOutcome, Connection as Link, FeederId, NewPhoneInvite,
    PhoneInvite, PhoneKey, RevokeAttempt, RevokeReason, Role, User,
};
use wellfed_formats::hex;
use wellfed_mirror::{AskError, Invite, MirrorEvent};
use wellfed_model::{Answer, Op, Refusal};
use wellfed_store::{NewPhone, Whose};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tracing::{Instrument, info, info_span, warn};

use crate::journal::Write;
use crate::{
    Caller, ClientError, CommandError, Error, FeederError, Permission, Result, Service, can, lock,
};

impl Service {
    /// An invite for the caller's own phone, which joins the feeder with
    /// `request.key` on the LAN. Only a phone's token gets one, for an
    /// owner's or member's account with no expiry on it or the token; the
    /// phone joins with its account's role. One key per token: once the
    /// invite is made, the token's other key on the feeder is revoked.
    ///
    /// The key is recorded before the invite is asked for. The invite is
    /// never resent: its secret is not kept, so a caller that lost the
    /// answer asks for a new one, and the old one expires unused.
    pub async fn invite_phone(
        &self,
        caller: &Caller,
        id: FeederId,
        request: NewPhoneInvite,
    ) -> Result<PhoneInvite> {
        let Some(token) = caller.token else {
            return Err(ClientError::TokenRequired.into());
        };
        let role = match caller.user.role {
            Role::Owner => ClientRole::Owner,
            Role::Member => ClientRole::Member,
            Role::Viewer => return Err(ClientError::DirectKeyRefused.into()),
        };
        let token_expires = self
            .shared
            .store
            .token(token)
            .await?
            .and_then(|token| token.expires);
        if caller.user.expires.is_some() || token_expires.is_some() {
            return Err(ClientError::DirectKeyRefused.into());
        }
        let row = self.live_row(id).await?;
        let key = request.key.0;
        let listed = self.list_clients(id).await?;
        if listed.iter().any(|client| client.key.0 == key) {
            return Err(ClientError::KeyTaken.into());
        }

        let store = &self.shared.store;
        let phone = NewPhone {
            feeder: id,
            key,
            user: caller.user.id,
            token,
            role,
            invited: self.shared.clock.now(),
        };
        let new = store.record_phone(&phone).await?;
        if !new {
            let theirs = store.phone(id, &key).await?.is_some_and(|recorded| {
                recorded.token == Some(token) && recorded.revoking.is_none()
            });
            if !theirs {
                return Err(ClientError::KeyTaken.into());
            }
        }

        let sent = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Invite,
                body: &request,
                key: None,
                op: Op::CreateInvite { role },
            })
            .await;
        let made = match sent {
            Ok(recorded) => match (recorded.outcome, recorded.invite) {
                (CommandOutcome::Invited { .. }, Some(made)) => made,
                (CommandOutcome::Refused { refusal }, _) => {
                    self.unrecord(new, id, &key).await?;
                    return Err(CommandError::Refused(refusal).into());
                }
                _ => return Err(CommandError::Unexpected.into()),
            },
            // The feeder may have made it: the key stays recorded, so a
            // phone that joins with it is still this account's.
            Err(error @ Error::Command(CommandError::Unknown)) => return Err(error),
            Err(error) => {
                self.unrecord(new, id, &key).await?;
                return Err(error);
            }
        };

        let replaced = Whose::Replaced {
            feeder: id,
            token,
            keep: key,
        };
        self.revoke_phones(replaced, RevokeReason::Replaced).await?;
        let invite = Invite {
            feeder: row.identity,
            id: made.id,
            secret: made.secret.0,
        };
        Ok(PhoneInvite {
            invite: invite.text(),
            role,
            seconds: made.seconds,
            address: row.address,
        })
    }

    /// The phone keys the server recorded: everyone's for an owner, the
    /// user's own for anyone else.
    pub async fn phones(&self, user: &User) -> Result<Vec<PhoneKey>> {
        let whose = (!can(user.role, Permission::Users)).then_some(user.id);
        Ok(self.shared.store.phones(whose).await?)
    }

    /// Forget a key this request recorded, once no invite can have been
    /// made for it.
    async fn unrecord(&self, new: bool, id: FeederId, key: &[u8; 32]) -> Result<()> {
        if new {
            self.shared.store.forget_client(id, key).await?;
        }
        Ok(())
    }

    /// Mark `whose` keys for revocation for `reason`, and start revoking
    /// them on each feeder that holds one.
    pub(crate) async fn revoke_phones(&self, whose: Whose, reason: RevokeReason) -> Result<()> {
        let now = self.shared.clock.now();
        let feeders = self.shared.store.revoke_phones(whose, reason, now).await?;
        for feeder in feeders {
            let service = self.clone();
            let span = info_span!(parent: None, "feeder", id = %feeder);
            self.tasks
                .spawn(async move { service.revoke_owed(feeder).await }.instrument(span));
        }
        Ok(())
    }

    /// Revoke each key still owed on the feeder, logging what is left: the
    /// next connect tries again.
    async fn revoke_owed(&self, id: FeederId) {
        if let Err(error) = self.try_revoke_owed(id).await {
            warn!(%error, "the phone keys owed a revoke wait for the next connect");
        }
    }

    async fn try_revoke_owed(&self, id: FeederId) -> Result<()> {
        // One pass per feeder at a time: an account change and a reconnect
        // may both start one.
        let turn = Arc::clone(lock(&self.revoking).entry(id).or_default());
        let _turn = turn.lock().await;
        let store = &self.shared.store;

        for phone in store.revoking(id).await? {
            let key = phone.key.0;
            let client = hex::encode(&key[..4]);
            let op = Op::RevokeClient { key };
            let attempt = match self.mirror.ask(id, self.request_id()?, op).await {
                Ok(Answer::Done { .. } | Answer::Refused(Refusal::NotFound)) => {
                    store.forget_client(id, &key).await?;
                    info!(%client, "a phone's key is revoked");
                    continue;
                }
                Ok(Answer::Refused(refusal)) => RevokeAttempt::Refused {
                    refusal: refusal.into(),
                },
                Ok(_) | Err(AskError::Unknown) => RevokeAttempt::Unknown,
                Err(AskError::Offline | AskError::NotRunning | AskError::Unsendable) => {
                    RevokeAttempt::Offline
                }
            };
            store.revoke_attempt(id, &key, &attempt).await?;
            warn!(%client, ?attempt, "a phone's key is not revoked yet");
            if attempt == RevokeAttempt::Offline {
                break;
            }
        }
        Ok(())
    }

    /// Revoke the keys owed on the feeder each time its session comes up.
    pub(crate) fn watch_revocations(&self, id: FeederId, mut updates: Receiver<MirrorEvent>) {
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
                match update {
                    Ok(MirrorEvent::Link(Link::Online)) => service.revoke_owed(id).await,
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {
                        updates = updates.resubscribe();
                        service.revoke_owed(id).await;
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        };
        self.tasks.spawn(watching.instrument(span));
    }
}

/// Why an account change revokes the account's phone keys, if it does:
/// a lower role (a viewer holds no key, a member's phone no owner's), or
/// an expiry where there was none or another.
pub(crate) fn revocation(before: &User, after: &User) -> Option<RevokeReason> {
    const fn rank(role: Role) -> u8 {
        match role {
            Role::Owner => 2,
            Role::Member => 1,
            Role::Viewer => 0,
        }
    }

    if rank(after.role) < rank(before.role) {
        return Some(RevokeReason::Demoted);
    }
    (after.expires.is_some() && after.expires != before.expires).then_some(RevokeReason::Expiring)
}

/// What the feeder answered a revoke of the server's own key, as a
/// removal reports it. A key already gone is revoked.
pub(crate) fn own_revoke(sent: Result<CommandOutcome>) -> Result<Option<RevokeAttempt>> {
    match sent {
        Ok(CommandOutcome::Done)
        | Ok(CommandOutcome::Refused {
            refusal: wellfed_api::FeederRefusal::NotFound,
        }) => Ok(None),
        Ok(CommandOutcome::Refused { refusal }) => Ok(Some(RevokeAttempt::Refused { refusal })),
        Ok(_) | Err(Error::Command(CommandError::Unknown | CommandError::Unexpected)) => {
            Ok(Some(RevokeAttempt::Unknown))
        }
        Err(Error::Command(CommandError::Offline) | Error::Feeder(FeederError::NotFound)) => {
            Ok(Some(RevokeAttempt::Offline))
        }
        Err(other) => Err(other),
    }
}
