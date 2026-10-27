//! What an owner administers on a feeder through the server: its clients
//! kept or revoked, its Wi-Fi against the store's generation, a tune, a
//! restart, and removing the feeder and adopting it again.

use wellfed_api::{
    AdoptFeeder, ClientOwner, ClientRole, Connection as Link, FeederId, FeederRefusal, Generation,
    KeyRemoval, PlayRequest, RevokeAttempt, TuneName, WifiChange, WifiCredentials,
};
use wellfed_server_testkit::feeder::{self, Running};
use wellfed_server_testkit::{TestService, ada, local, until};
use wellfed_service::{Caller, ClientError, CommandError, Error, FeederError, Invalid, Service};

async fn started(role: ClientRole) -> (Running, TestService, FeederId) {
    let sim = feeder::start().await;
    let server = TestService::start().await;
    let id = server.adopt_as(&sim, &sim.address.to_string(), role).await;
    server.link(id, Link::Online).await;
    (sim, server, id)
}

fn owner() -> Caller {
    ada().into()
}

fn home(psk: &str) -> WifiChange {
    WifiChange {
        wifi: Some(WifiCredentials {
            ssid: "home".into(),
            psk: psk.into(),
        }),
    }
}

/// The feeder's status, read now.
async fn status(service: &Service, id: FeederId) -> wellfed_model::Status {
    let detail = service.feeder(id).await.unwrap();
    let snapshot = detail.status.unwrap();
    assert!(snapshot.live, "read for this request");
    snapshot.status
}

#[test]
fn an_unowned_client_is_kept_or_revoked_and_the_servers_own_key_is_not_revoked() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let cli = *feeder::owner().public();
        let owner_of = async |key: [u8; 32]| {
            let clients = service.clients(id).await.unwrap();
            let client = clients.iter().find(|client| client.key.0 == key);
            client.map(|client| client.owner.clone())
        };
        assert_eq!(owner_of(cli).await, Some(ClientOwner::Unowned));

        service.keep_client(id, cli).await.unwrap();
        assert_eq!(owner_of(cli).await, Some(ClientOwner::Kept));

        // Another client, joined at the feeder without the server.
        let invite = feeder::invite(&sim, ClientRole::Member).await;
        let stranger = feeder::phone_key([9; 32]);
        feeder::join_phone(&sim, &invite, [9; 32]).await;
        assert_eq!(owner_of(stranger).await, Some(ClientOwner::Unowned));
        service
            .revoke_client(&owner(), id, stranger, Some([1; 16]))
            .await
            .unwrap();
        assert_eq!(owner_of(stranger).await, None);
        assert!(!feeder::client_keys(&sim).await.contains(&stranger));

        let clients = service.clients(id).await.unwrap();
        let own = clients
            .iter()
            .find(|client| client.owner == ClientOwner::Server)
            .unwrap()
            .key
            .0;
        let refused = service
            .revoke_client(&owner(), id, own, Some([2; 16]))
            .await;
        assert!(
            matches!(refused, Err(Error::Client(ClientError::OwnKey))),
            "{refused:?}"
        );
        assert!(feeder::client_keys(&sim).await.contains(&own));
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn wifi_changes_against_the_stores_generation_and_a_stale_base_is_refused() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let base = status(service, id).await.wifi_generation;

        for (psk, want) in [
            ("short", Invalid::Psk),
            ("tab\tin a passphrase", Invalid::Psk),
        ] {
            let result = service
                .put_wifi(&owner(), id, base, home(psk), Some([1; 16]))
                .await;
            assert!(
                matches!(result, Err(Error::Invalid(got)) if got == want),
                "{result:?}"
            );
        }
        let unnamed = WifiChange {
            wifi: Some(WifiCredentials {
                ssid: String::new(),
                psk: "long enough".into(),
            }),
        };
        let result = service
            .put_wifi(&owner(), id, base, unnamed, Some([1; 16]))
            .await;
        assert!(
            matches!(result, Err(Error::Invalid(Invalid::Ssid))),
            "{result:?}"
        );

        service
            .put_wifi(&owner(), id, base, home("long enough"), Some([2; 16]))
            .await
            .unwrap();
        let moved = status(service, id).await.wifi_generation;
        assert_eq!(moved, Generation(base.0 + 1));

        let stale = service
            .put_wifi(&owner(), id, base, home("another one"), Some([3; 16]))
            .await;
        assert!(
            matches!(
                stale,
                Err(Error::Command(CommandError::Refused(FeederRefusal::Stale)))
            ),
            "{stale:?}"
        );
        let cleared = WifiChange { wifi: None };
        service
            .put_wifi(&owner(), id, moved, cleared, Some([4; 16]))
            .await
            .unwrap();
        server.shutdown().await;
        sim.stop().await;
    });
}

/// The firmware's loop does not step the speaker yet, so a tune is
/// refused `unavailable`; the server passes the refusal on as it would
/// `muted`.
#[test]
fn a_tune_the_feeder_cannot_play_is_refused_with_its_reason() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let chime = PlayRequest {
            tune: TuneName::Chime,
        };

        let played = service.play(&owner(), id, chime, Some([1; 16])).await;

        assert!(
            matches!(
                played,
                Err(Error::Command(CommandError::Refused(
                    FeederRefusal::Unavailable
                )))
            ),
            "{played:?}"
        );
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_restart_runs_once_and_the_server_reconnects_after_it() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let boots = status(service, id).await.boot_count;

        service.reboot(&owner(), id, Some([1; 16])).await.unwrap();

        until("the server is back on the restarted feeder", || async {
            let detail = service.feeder(id).await.unwrap();
            detail
                .status
                .is_some_and(|snapshot| snapshot.live && snapshot.status.boot_count == boots + 1)
        })
        .await;
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_removed_feeder_is_archived_its_key_revoked_and_adopted_again_under_its_id() {
    local(async {
        let (sim, server, id) = started(ClientRole::Owner).await;
        let service = server.service();
        let own = service
            .clients(id)
            .await
            .unwrap()
            .into_iter()
            .find(|client| client.owner == ClientOwner::Server)
            .unwrap()
            .key
            .0;

        let removal = service.remove_feeder(&owner(), id).await.unwrap();

        assert_eq!(removal.key, KeyRemoval::Revoked);
        assert!(removal.feeder.archived.is_some());
        assert!(!feeder::client_keys(&sim).await.contains(&own));
        let listed = service.feeders().await.unwrap();
        assert!(
            listed
                .iter()
                .any(|feeder| feeder.id == id && feeder.archived.is_some())
        );
        assert!(!server.bodies(id).await.is_empty(), "its history stays");
        let refused = service.reboot(&owner(), id, Some([1; 16])).await;
        assert!(
            matches!(refused, Err(Error::Feeder(FeederError::Archived))),
            "{refused:?}"
        );

        let again = service
            .adopt(AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Owner).await,
                name: "again".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap();
        assert_eq!(again.id, id);
        assert_eq!(again.archived, None);
        server.link(id, Link::Online).await;
        service.reboot(&owner(), id, Some([2; 16])).await.unwrap();
        server.shutdown().await;
        sim.stop().await;
    });
}

/// A `server` key may not revoke, its own included: the feeder is still
/// removed, and the answer says the key is left for the owner to revoke.
#[test]
fn a_removal_the_feeder_refuses_still_archives_and_says_the_key_stays() {
    local(async {
        let (sim, server, id) = started(ClientRole::Server).await;
        let service = server.service();

        let removal = service.remove_feeder(&owner(), id).await.unwrap();

        let refused = RevokeAttempt::Refused {
            refusal: FeederRefusal::Forbidden,
        };
        assert_eq!(removal.key, KeyRemoval::Failed { attempt: refused });
        assert!(removal.feeder.archived.is_some());
        let held = feeder::client_keys(&sim).await;
        assert_eq!(held.len(), 2, "the owner's and the server's");
        server.shutdown().await;
        sim.stop().await;
    });
}
