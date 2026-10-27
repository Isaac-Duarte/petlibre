//! Phones' own keys on a feeder: invites made only for an owner's or
//! member's token with no expiry, recorded as the account's, listed as its
//! client, and revoked on every feeder when the account is deleted,
//! demoted or given an expiry, or the token is revoked, retried at the
//! feeder's next connect until it lands; a key whose invite is still live
//! is revoked once it joins, and forgotten at the invite's deadline.

use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{
    ClientOwner, ClientRole, Connection as Link, FeederId, FeederRefusal, Hex, NewPhoneInvite,
    NewToken, NewUser, PhoneKey, RevokeAttempt, RevokeReason, Role, User, UserPatch,
};
use wellfed_server_testkit::feeder::{self, Flash, Running};
use wellfed_server_testkit::fixed::{FixedClock, ManualClock};
use wellfed_server_testkit::{TestService, ada_setup, local, until};
use wellfed_service::{Caller, ClientError, Error, Service};

/// A phone's secret key, by number.
fn secret(n: u8) -> [u8; 32] {
    [n; 32]
}

/// The owner, as the account changes' caller.
fn owner() -> Caller {
    wellfed_server_testkit::ada().into()
}

fn invite_for(n: u8) -> NewPhoneInvite {
    NewPhoneInvite {
        key: Hex(feeder::phone_key(secret(n))),
    }
}

/// A user signed in on a phone: their account and a token of theirs.
async fn phone_user(service: &Service, name: &str, role: Role) -> Caller {
    let user = match name {
        "ada" => service.setup(ada_setup()).await.unwrap().0,
        _ => {
            let new = NewUser {
                name: name.into(),
                password: "temporary pass".into(),
                role,
                expires: None,
            };
            let owner = wellfed_server_testkit::ada().into();
            service.create_user(&owner, new).await.unwrap()
        }
    };
    token_of(service, user, None).await
}

async fn token_of(service: &Service, user: User, expires: Option<jiff::Timestamp>) -> Caller {
    let login = Caller::from(user);
    let new = NewToken {
        name: "phone".into(),
        expires,
    };
    let token = service.create_token(&login, new).await.unwrap();
    Caller {
        user: login.user,
        token: Some(token.token.id),
    }
}

/// The phone numbered `n` joins the feeder as `caller`'s: the role it got.
async fn join(
    service: &Service,
    sim: &Running,
    id: FeederId,
    caller: &Caller,
    n: u8,
) -> ClientRole {
    let invite = service
        .invite_phone(caller, id, invite_for(n))
        .await
        .unwrap();
    feeder::join_phone(sim, &invite.invite, secret(n)).await
}

/// Until the feeder no longer holds phone `n`'s key and the server no
/// longer records it.
async fn revoked(service: &Service, sim: &Running, n: u8) {
    let key = feeder::phone_key(secret(n));
    until("the phone's key is revoked", || async {
        let held = feeder::client_keys(sim).await.contains(&key);
        let owner = wellfed_server_testkit::ada();
        let recorded = service.phones(&owner).await.unwrap();
        !held && !recorded.iter().any(|phone| phone.key.0 == key)
    })
    .await;
}

async fn started(role: ClientRole) -> (Running, TestService, FeederId) {
    let sim = feeder::start().await;
    let server = TestService::start().await;
    let id = server.adopt_as(&sim, &sim.address.to_string(), role).await;
    server.link(id, Link::Online).await;
    (sim, server, id)
}

#[test]
fn an_owners_and_a_members_phone_join_with_their_roles_and_are_listed_as_theirs() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;

        assert_eq!(join(service, &sim, id, &ada, 1).await, ClientRole::Owner);
        assert_eq!(join(service, &sim, id, &bob, 2).await, ClientRole::Member);

        let clients = service.clients(id).await.unwrap();
        let owner_of = |n: u8| {
            let key = feeder::phone_key(secret(n));
            let client = clients.iter().find(|client| client.key.0 == key).unwrap();
            client.owner.clone()
        };
        let bobs = ClientOwner::Phone {
            user: bob.user.id,
            token: bob.token,
            revoking: None,
        };
        assert_eq!(owner_of(2), bobs);
        assert!(matches!(owner_of(1), ClientOwner::Phone { user, .. } if user == ada.user.id));
        assert!(
            clients
                .iter()
                .any(|client| client.owner == ClientOwner::Server)
        );
        let owners_cli = feeder::owner().public().to_owned();
        let cli = clients
            .iter()
            .find(|client| client.key.0 == owners_cli)
            .unwrap();
        assert_eq!(
            cli.owner,
            ClientOwner::Unowned,
            "the server did not invite it"
        );

        // A member sees their own phone's key; an owner sees both.
        let bobs_keys = service.phones(&bob.user).await.unwrap();
        assert_eq!(bobs_keys.len(), 1);
        assert_eq!(bobs_keys[0].role, ClientRole::Member);
        assert_eq!(service.phones(&ada.user).await.unwrap().len(), 2);
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn only_a_token_of_an_account_with_no_expiry_gets_a_phone_invite() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let refused = async |caller: &Caller| service.invite_phone(caller, id, invite_for(3)).await;

        let browser = Caller::from(ada.user.clone());
        let tomorrow = FixedClock.now() + jiff::SignedDuration::from_hours(24);
        let expiring_token = token_of(service, ada.user.clone(), Some(tomorrow)).await;
        let sitter = phone_user(service, "cy", Role::Member).await;
        let patch = UserPatch {
            expires: Some(Some(tomorrow)),
            ..UserPatch::default()
        };
        service
            .update_user(&ada, None, sitter.user.id, patch)
            .await
            .unwrap();
        let sitter = Caller {
            user: User {
                expires: Some(tomorrow),
                ..sitter.user
            },
            ..sitter
        };
        let viewer = phone_user(service, "dee", Role::Viewer).await;

        let cases = [
            (&browser, ClientError::TokenRequired),
            (&expiring_token, ClientError::DirectKeyRefused),
            (&sitter, ClientError::DirectKeyRefused),
            (&viewer, ClientError::DirectKeyRefused),
        ];
        for (caller, want) in cases {
            let result = refused(caller).await;
            assert!(
                matches!(result, Err(Error::Client(got)) if got == want),
                "{want:?}: {result:?}"
            );
        }
        assert!(service.phones(&ada.user).await.unwrap().is_empty());
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_key_already_enrolled_or_another_phones_is_refused() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;

        let cli = NewPhoneInvite {
            key: Hex(*feeder::owner().public()),
        };
        let enrolled = service.invite_phone(&ada, id, cli).await;
        assert!(matches!(
            enrolled,
            Err(Error::Client(ClientError::KeyTaken))
        ));

        service.invite_phone(&ada, id, invite_for(4)).await.unwrap();
        let theirs = service.invite_phone(&bob, id, invite_for(4)).await;
        assert!(matches!(theirs, Err(Error::Client(ClientError::KeyTaken))));
        let again = service.invite_phone(&ada, id, invite_for(4)).await;
        assert!(again.is_ok(), "a lost invite is asked for again: {again:?}");
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The feeder lets no `server` key invite: the refusal is the feeder's,
/// and the key it was for is not left recorded.
#[test]
fn a_server_role_key_is_refused_the_invite_and_records_nothing() {
    local(async {
        let (sim, server, id) = started(ClientRole::Server).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;

        let refused = service.invite_phone(&ada, id, invite_for(5)).await;

        assert!(
            matches!(refused, Err(Error::Refused(FeederRefusal::Forbidden))),
            "{refused:?}"
        );
        assert!(service.phones(&ada.user).await.unwrap().is_empty());
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn deleting_demoting_or_expiring_an_account_revokes_its_phones_keys() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;
        let cy = phone_user(service, "cy", Role::Member).await;
        let dee = phone_user(service, "dee", Role::Owner).await;
        let eve = phone_user(service, "eve", Role::Member).await;
        for (caller, n) in [(&ada, 10), (&bob, 11), (&cy, 12), (&dee, 13), (&eve, 14)] {
            join(service, &sim, id, caller, n).await;
        }

        let tomorrow = FixedClock.now() + jiff::SignedDuration::from_hours(24);
        service.delete_user(&owner(), bob.user.id).await.unwrap();
        let viewer = UserPatch {
            role: Some(Role::Viewer),
            ..UserPatch::default()
        };
        service
            .update_user(&ada, None, cy.user.id, viewer)
            .await
            .unwrap();
        let member = UserPatch {
            role: Some(Role::Member),
            ..UserPatch::default()
        };
        service
            .update_user(&ada, None, dee.user.id, member)
            .await
            .unwrap();
        let expiring = UserPatch {
            expires: Some(Some(tomorrow)),
            ..UserPatch::default()
        };
        service
            .update_user(&ada, None, eve.user.id, expiring)
            .await
            .unwrap();

        for n in [11, 12, 13, 14] {
            revoked(service, &sim, n).await;
        }
        let kept = feeder::phone_key(secret(10));
        assert!(
            feeder::client_keys(&sim).await.contains(&kept),
            "ada's stays"
        );
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn revoking_a_token_or_inviting_another_key_revokes_the_tokens_key() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        join(service, &sim, id, &ada, 20).await;

        // One key per token: a new phone's invite revokes the old key.
        join(service, &sim, id, &ada, 21).await;
        revoked(service, &sim, 20).await;

        let token = ada.token.unwrap();
        service.revoke_token(&ada, token).await.unwrap();
        revoked(service, &sim, 21).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A feeder offline when the account changes keeps the key until it
/// reconnects; meanwhile the key shows the revoke owed and its last
/// attempt.
#[test]
fn a_revoke_owed_to_an_offline_feeder_lands_when_it_reconnects() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address;
        let server = TestService::start().await;
        let id = server
            .adopt_as(&sim, &address.to_string(), ClientRole::Owner)
            .await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;
        join(service, &sim, id, &bob, 30).await;

        sim.stop().await;
        server.link(id, Link::Offline).await;
        service.delete_user(&owner(), bob.user.id).await.unwrap();
        let owed = |phones: &[PhoneKey]| {
            phones.iter().any(|phone| {
                phone.revoking.as_ref().is_some_and(|revoking| {
                    revoking.reason == RevokeReason::Deleted
                        && revoking.last == Some(RevokeAttempt::Offline)
                })
            })
        };
        until("the revoke is reported owed", || async {
            owed(&service.phones(&ada.user).await.unwrap())
        })
        .await;

        let sim = feeder::restart(&flash, address).await;
        revoked(service, &sim, 30).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// Whether a phone key is owed a revoke whose invite may still be redeemed.
fn waiting_for_its_invite(phones: &[PhoneKey]) -> bool {
    phones.iter().any(|phone| {
        let last = phone.revoking.as_ref().and_then(|revoking| revoking.last);
        matches!(last, Some(RevokeAttempt::InviteLive { .. }))
    })
}

/// The feeder holds no key for an invite not yet redeemed, and does not
/// bind the invite to a key: a revoke owed before the phone joins waits
/// for it, and lands when it joins.
#[test]
fn a_key_revoked_before_its_phone_joins_is_revoked_once_it_joins() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;
        let invite = service
            .invite_phone(&bob, id, invite_for(40))
            .await
            .unwrap();

        service.delete_user(&owner(), bob.user.id).await.unwrap();
        until("the revoke waits for the invite", || async {
            waiting_for_its_invite(&service.phones(&ada.user).await.unwrap())
        })
        .await;
        feeder::join_phone(&sim, &invite.invite, secret(40)).await;

        revoked(service, &sim, 40).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// An owed key whose invite was never redeemed is forgotten once the
/// invite's deadline has passed, tried again then.
#[test]
fn a_key_that_never_joins_is_settled_at_its_invites_deadline() {
    local(async {
        let sim = feeder::start().await;
        let clock = ManualClock::new();
        let server = TestService::start_with_clock(Arc::new(clock.clone())).await;
        let id = server
            .adopt_as(&sim, &sim.address.to_string(), ClientRole::Owner)
            .await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;
        let invite = service
            .invite_phone(&bob, id, invite_for(41))
            .await
            .unwrap();

        service.delete_user(&owner(), bob.user.id).await.unwrap();
        until("the revoke waits for the invite", || async {
            waiting_for_its_invite(&service.phones(&ada.user).await.unwrap())
        })
        .await;
        clock.advance(Duration::from_secs(u64::from(invite.seconds) + 120));

        until("the key is forgotten", || async {
            service.phones(&ada.user).await.unwrap().is_empty()
        })
        .await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A revoke owed when the server stops is still owed when it starts again,
/// and lands when the feeder returns.
#[test]
fn an_owed_revoke_survives_a_server_restart() {
    local(async {
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address;
        let server = TestService::start().await;
        let id = server
            .adopt_as(&sim, &address.to_string(), ClientRole::Owner)
            .await;
        server.link(id, Link::Online).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let cy = phone_user(service, "cy", Role::Member).await;
        join(service, &sim, id, &cy, 42).await;

        sim.stop().await;
        server.link(id, Link::Offline).await;
        service.delete_user(&owner(), cy.user.id).await.unwrap();
        until("the revoke is owed", || async {
            let phones = service.phones(&ada.user).await.unwrap();
            phones.iter().any(|phone| {
                let last = phone.revoking.as_ref().and_then(|revoking| revoking.last);
                last == Some(RevokeAttempt::Offline)
            })
        })
        .await;

        let server = TestService::reopen(server.stop().await).await;
        let sim = feeder::restart(&flash, address).await;
        revoked(server.service(), &sim, 42).await;
        server.shutdown().await;
        sim.stop().await;
    });
}

/// Removing a feeder tries the owed revokes first, and names the keys
/// still owed: here one whose invite may yet be redeemed.
#[test]
fn a_removal_names_the_phone_keys_still_owed() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let ada = phone_user(service, "ada", Role::Owner).await;
        let bob = phone_user(service, "bob", Role::Member).await;
        service
            .invite_phone(&bob, id, invite_for(43))
            .await
            .unwrap();
        service.delete_user(&owner(), bob.user.id).await.unwrap();

        let removal = service.remove_feeder(&ada, id).await.unwrap();

        let owed: Vec<_> = removal
            .phones_owed
            .iter()
            .map(|phone| phone.key.0)
            .collect();
        assert_eq!(owed, [feeder::phone_key(secret(43))]);
        server.shutdown().await;
        sim.stop().await;
    });
}
