//! Phones' own keys on the feeders, and revoking them.
//!
//! An owner's or member's phone keeps a key on every feeder, for the LAN
//! while the server is down. The server makes the invite it joins with,
//! recording the key, account and token first, so a phone that joins with
//! the key it named is attributed even when the invite's answer is lost.
//! A viewer, and an account or token with an expiry, holds no key: the
//! server alone enforces its limits. Revoking them is [`revoke`]'s.

use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    ClientRole, CommandKind, CommandOutcome, FeederId, NewPhoneInvite, PhoneInvite, PhoneKey,
    RevokeReason, Role, User,
};
use wellfed_mirror::Invite;
use wellfed_model::Op;
use wellfed_store::{NewPhone, Recorded, Whose};

mod revoke;

pub(crate) use revoke::{Revocations, own_revoke, revocation};

use crate::feeders::journal::Write;
use crate::{
    Caller, ClientError, CommandError, Error, InternalError, Permission, Result, Service, can,
};

/// The deadline assumed for an invite while its answer is pending or was
/// lost: the longest an answer can state (`seconds` is a `u16`).
const INVITE_BOUND: SignedDuration = SignedDuration::from_secs(65_535);
/// Added to the deadline the feeder states, which it counts from when it
/// made the invite, a little after the server last read its clock.
const INVITE_MARGIN: SignedDuration = SignedDuration::from_secs(60);

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
                    return Err(Error::Refused(refusal));
                }
                _ => return Err(InternalError::Unexpected.into()),
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
}
