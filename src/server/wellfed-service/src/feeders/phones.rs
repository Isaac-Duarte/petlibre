//! Phones' own keys on the feeders, and revoking them.
//!
//! An owner's or member's phone keeps a key on every feeder, for the LAN
//! while the server is down. The server makes the invite it joins with,
//! recording the key, account and token first, so a phone that joins with
//! the key it named is attributed even when the invite's answer is lost.
//! A viewer, and an account or token with an expiry, holds no key: the
//! server alone enforces its limits.
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

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    ClientRole, CommandKind, CommandOutcome, Connection as Link, EventBody, FeederId,
    NewPhoneInvite, PhoneInvite, PhoneKey, RevokeAttempt, RevokeReason, Role, User,
};
use wellfed_formats::hex;
use wellfed_mirror::{AskError, Invite, MirrorEvent};
use wellfed_model::{Answer, Op, Refusal};
use wellfed_store::{NewPhone, Recorded, Whose};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tracing::{Instrument, info, info_span, warn};

use crate::feeders::journal::Write;
use crate::{
    Caller, ClientError, CommandError, Error, FeederError, Permission, Result, Service, can, lock,
};

/// The deadline assumed for an invite while its answer is pending or was
/// lost: the longest an answer can state (`seconds` is a `u16`).
const INVITE_BOUND: SignedDuration = SignedDuration::from_secs(65_535);
/// Added to the deadline the feeder states, which it counts from when it
/// made the invite, a little after the server last read its clock.
const INVITE_MARGIN: SignedDuration = SignedDuration::from_secs(60);

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
    /// An invite for the caller's own phone, which joins the feeder with
    /// `request.key` on the LAN. Only a phone's token gets one, for an
    /// owner's or member's account with no expiry on it or the token; the
    /// phone joins with its account's role. One key per token: once the
    /// invite is made, the token's other key on the feeder is revoked.
    ///
    /// The key is recorded before the invite is asked for. The invite is
    /// never resent: its secret is not kept, so a caller that lost the
    /// answer asks for a new one, and the old one expires unused. An
    /// account that lost its key while the invite was made gets no invite,
    /// and the key is revoked should it join.
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
        let now = self.shared.clock.now();
        let bound = now + INVITE_BOUND;
        let phone = NewPhone {
            feeder: id,
            key,
            user: caller.user.id,
            token,
            role,
            invited: now,
            invite_until: bound,
        };
        // The deadline to restore if no invite was made: none for a new key.
        let before = match store.record_phone(&phone).await? {
            Recorded::New => None,
            Recorded::Again { until } => Some(until),
            Recorded::Taken => return Err(ClientError::KeyTaken.into()),
        };

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
                    self.unrecord(before, id, &key, bound).await?;
                    return Err(CommandError::Refused(refusal).into());
                }
                _ => return Err(CommandError::Unexpected.into()),
            },
            // The feeder may have made it: the key stays recorded under the
            // bound, so a phone that joins with it is still this account's.
            Err(error @ Error::Command(CommandError::Unknown)) => return Err(error),
            Err(error) => {
                self.unrecord(before, id, &key, bound).await?;
                return Err(error);
            }
        };
        let deadline = self.shared.clock.now()
            + SignedDuration::from_secs(i64::from(made.seconds))
            + INVITE_MARGIN;
        let until = before.map_or(deadline, |before| before.max(deadline));
        store.settle_invite_until(id, &key, bound, until).await?;

        // The account may have lost its key while the invite was made.
        self.reconcile_phones().await?;
        let standing = store
            .phone(id, &key)
            .await?
            .is_some_and(|phone| phone.revoking.is_none());
        if !standing {
            self.spawn_revoke(id);
            return Err(ClientError::DirectKeyRefused.into());
        }

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

    /// Undo this request's record once no invite was made for it: forget a
    /// key it recorded, or put back the deadline a key asked again had,
    /// unless another request for the key has raised it since.
    async fn unrecord(
        &self,
        before: Option<Timestamp>,
        id: FeederId,
        key: &[u8; 32],
        bound: Timestamp,
    ) -> Result<()> {
        let store = &self.shared.store;
        match before {
            None => store.unrecord_phone(id, key, bound).await?,
            Some(until) => {
                store.settle_invite_until(id, key, bound, until).await?;
            }
        }
        Ok(())
    }

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

    fn spawn_revoke(&self, feeder: FeederId) {
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
        Ok(_) | Err(Error::Command(CommandError::Unknown | CommandError::Unexpected)) => {
            Ok(Some(RevokeAttempt::Unknown))
        }
        Err(Error::Command(CommandError::Offline) | Error::Feeder(FeederError::NotFound)) => {
            Ok(Some(RevokeAttempt::Offline))
        }
        Err(other) => Err(other),
    }
}
