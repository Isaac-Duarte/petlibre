//! Revoking phones' keys on the feeders.
//!
//! Deleting, demoting or putting an expiry on an account, or revoking a
//! token, marks its keys for revocation on every feeder, and a reconcile at
//! start and on each connect marks any key such a change left unmarked.
//! Each feeder revokes them at once when online, again when a client joins
//! it, and again each time it reconnects, until it has dropped them; until
//! then each key shows the last attempt, so an owner sees which feeders
//! still hold one. A revoke is idempotent, so each attempt takes a fresh
//! request id and needs no journal entry. The feeder does not bind an
//! invite to a key, so `not_found` settles a revoke only once the key's
//! invite can no longer be redeemed; before then it is tried again at the
//! invite's deadline.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use wellfed_api::{
    CommandOutcome, Connection as Link, EventBody, FeederId, RevokeAttempt, RevokeReason, Role,
    User,
};
use wellfed_formats::hex;
use wellfed_mirror::{AskError, MirrorEvent};
use wellfed_model::{Answer, Op, Refusal};
use wellfed_store::Whose;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tracing::{Instrument, info, info_span, warn};

use crate::{CommandError, Error, InternalError, Resource, Result, Service, lock};

/// What the owed phone-key revokes keep between passes.
#[derive(Clone, Default)]
pub(crate) struct Revocations {
    /// Held while a feeder's owed revokes are sent, so an account change
    /// and a reconnect take turns.
    turns: Arc<Mutex<HashMap<FeederId, Arc<tokio::sync::Mutex<()>>>>>,
    /// When each feeder's owed revokes are next tried for an invite's
    /// deadline, so one wake serves every key waiting on that feeder.
    wakes: Arc<Mutex<HashMap<FeederId, Timestamp>>>,
}

impl Service {
    /// Mark `whose` keys for revocation for `reason`, and start revoking
    /// them on each feeder that holds one.
    pub(crate) async fn revoke_phones(&self, whose: Whose, reason: RevokeReason) -> Result<()> {
        let now = self.shared.clock.now();
        let feeders = self.shared.store.revoke_phones(whose, reason, now).await?;
        for feeder in feeders {
            self.spawn_revoke(feeder);
        }
        Ok(())
    }

    /// Mark the keys any account or token change left unmarked, and start
    /// revoking them.
    pub(crate) async fn reconcile_phones(&self) -> Result<()> {
        let now = self.shared.clock.now();
        let feeders = self.shared.store.reconcile_phones(now).await?;
        for feeder in feeders {
            self.spawn_revoke(feeder);
        }
        Ok(())
    }

    pub(super) fn spawn_revoke(&self, feeder: FeederId) {
        let service = self.clone();
        let span = info_span!(parent: None, "feeder", id = %feeder);
        self.tasks
            .spawn(async move { service.revoke_owed(feeder).await }.instrument(span));
    }

    /// Revoke each key still owed on the feeder, logging what is left: a
    /// join, the invite's deadline or the next connect tries again.
    pub(crate) async fn revoke_owed(&self, id: FeederId) {
        if let Err(error) = self.try_revoke_owed(id).await {
            warn!(%error, "the phone keys owed a revoke wait for the next connect");
        }
    }

    async fn try_revoke_owed(&self, id: FeederId) -> Result<()> {
        // One pass per feeder at a time: an account change and a reconnect
        // may both start one.
        let turn = Arc::clone(lock(&self.feeders.revocations.turns).entry(id).or_default());
        let _turn = turn.lock().await;
        let store = &self.shared.store;

        for phone in store.revoking(id).await? {
            let key = phone.key.0;
            let client = hex::encode(&key[..4]);
            let op = Op::RevokeClient { key };
            let attempt = match self.mirror.ask(id, self.request_id()?, op).await {
                Ok(Answer::Done { .. }) => {
                    store.forget_client(id, &key).await?;
                    info!(%client, "a phone's key is revoked");
                    continue;
                }
                Ok(Answer::Refused(Refusal::NotFound))
                    if self.shared.clock.now() >= phone.invite_until =>
                {
                    store.forget_client(id, &key).await?;
                    info!(%client, "a phone's key never joined, and its invite has expired");
                    continue;
                }
                Ok(Answer::Refused(Refusal::NotFound)) => {
                    let until = phone.invite_until;
                    self.wake_at(id, until);
                    RevokeAttempt::InviteLive { until }
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

    /// Revoke the keys owed on the feeder again at `at`, an invite's
    /// deadline, unless a wake no later is already set.
    fn wake_at(&self, id: FeederId, at: Timestamp) {
        {
            let mut wakes = lock(&self.feeders.revocations.wakes);
            if wakes.get(&id).is_some_and(|set| *set <= at) {
                return;
            }
            wakes.insert(id, at);
        }
        let service = self.clone();
        let cancel = self.cancel.child_token();
        let span = info_span!(parent: None, "feeder", id = %id);
        let wake = async move {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = service.shared.clock.sleep_until(at) => {}
            }
            {
                let mut wakes = lock(&service.feeders.revocations.wakes);
                if wakes.get(&id) == Some(&at) {
                    wakes.remove(&id);
                }
            }
            service.revoke_owed(id).await;
        };
        self.tasks.spawn(wake.instrument(span));
    }

    /// Revoke the keys owed on the feeder each time its session comes up,
    /// and each time a client joins it (an owed key's invite redeemed) or
    /// its history has a gap that may hide one.
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
                    Ok(MirrorEvent::Link(Link::Online)) => {
                        if let Err(error) = service.reconcile_phones().await {
                            warn!(%error, "the phone keys could not be reconciled");
                        }
                        service.revoke_owed(id).await;
                    }
                    Ok(MirrorEvent::Event(event))
                        if matches!(event.body, EventBody::Enrolled { .. }) =>
                    {
                        service.revoke_owed(id).await;
                    }
                    Ok(MirrorEvent::Gap(_)) => service.revoke_owed(id).await,
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
        Ok(_)
        | Err(Error::Command(CommandError::Unknown) | Error::Internal(InternalError::Unexpected)) => {
            Ok(Some(RevokeAttempt::Unknown))
        }
        Err(Error::Command(CommandError::Offline) | Error::NotFound(Resource::Feeder)) => {
            Ok(Some(RevokeAttempt::Offline))
        }
        Err(other) => Err(other),
    }
}
