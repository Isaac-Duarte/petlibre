//! A feeder's clients, the phones' own keys and an owner's administration
//! over HTTP: who may call each route, the codes each refusal maps to, and
//! a removed feeder refused `feeder_archived`.

use uuid::Uuid;
use wellfed_api::{
    AdoptFeeder, ClientOwner, ClientRole, Connection, ErrorCode, FeederId, Hex, KeyRemoval,
    NewToken, NewUser, Role, TuneName, WifiChange, WifiCredentials,
};
use wellfed_server_client::{Auth, Client, StatusCode};
use wellfed_server_testkit::feeder::{self, Running};
use wellfed_server_testkit::{TestServer, ada_setup, local, refused, refused_for, until};

fn key(n: u128) -> Uuid {
    Uuid::from_u128(0x0192c5d0_5555_7000_8000_000000000000 + n)
}

/// The owner's client after setup, and a feeder it adopted with an owner's
/// invite, online.
async fn adopted(server: &TestServer, sim: &Running) -> (Client, FeederId) {
    let mut owner = server.client();
    owner.setup(&ada_setup()).await.unwrap();
    let id = owner
        .adopt(&AdoptFeeder {
            invite: feeder::invite(sim, ClientRole::Owner).await,
            name: "kitchen".into(),
            address: Some(sim.address.to_string()),
        })
        .await
        .unwrap()
        .id;
    until("the server connects to the feeder", || async {
        owner.feeder(id).await.unwrap().feeder.connection == Connection::Online
    })
    .await;
    (owner, id)
}

/// A new account's token client.
async fn phone(server: &TestServer, owner: &Client, name: &str, role: Role) -> Client {
    let new = NewUser {
        name: name.into(),
        password: "temporary pass".into(),
        role,
        expires: None,
    };
    owner.create_user(&new).await.unwrap();
    let mut session = server.client();
    let login = wellfed_api::Credentials {
        name: name.into(),
        password: "temporary pass".into(),
    };
    session.login(&login).await.unwrap();
    let token = NewToken {
        name: "phone".into(),
        expires: None,
    };
    let created = session.create_token(&token).await.unwrap();
    server.client().with_auth(Some(Auth::Token(created.secret)))
}

fn home() -> WifiChange {
    WifiChange {
        wifi: Some(WifiCredentials {
            ssid: "home".into(),
            psk: "long enough".into(),
        }),
    }
}

#[test]
fn a_phone_joins_by_its_invite_and_an_owner_keeps_or_revokes_clients() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let (owner, id) = adopted(&server, &sim).await;
        let bob = phone(&server, &owner, "bob", Role::Member).await;
        let dee = phone(&server, &owner, "dee", Role::Viewer).await;

        let bobs_key = Hex(feeder::phone_key([5; 32]));
        let invite = bob.invite_phone(id, bobs_key).await.unwrap();
        assert_eq!(invite.role, ClientRole::Member);
        assert_eq!(
            feeder::join_phone(&sim, &invite.invite, [5; 32]).await,
            ClientRole::Member
        );
        assert_eq!(bob.phones().await.unwrap().len(), 1);

        refused(
            owner
                .invite_phone(id, Hex(feeder::phone_key([6; 32])))
                .await,
            StatusCode::FORBIDDEN,
            ErrorCode::TokenRequired,
        );
        refused(
            dee.invite_phone(id, Hex(feeder::phone_key([7; 32]))).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        refused(
            bob.clients(id).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );

        let clients = owner.clients(id).await.unwrap();
        let bobs = clients
            .iter()
            .find(|client| client.key == bobs_key)
            .unwrap();
        assert!(matches!(bobs.owner, ClientOwner::Phone { .. }));
        let cli = Hex(*feeder::owner().public());
        owner.keep_client(id, cli).await.unwrap();
        let own = clients
            .iter()
            .find(|client| client.owner == ClientOwner::Server)
            .unwrap()
            .key;
        refused(
            owner.revoke_client(id, own, key(1)).await,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        );

        owner.revoke_client(id, bobs_key, key(2)).await.unwrap();
        let clients = owner.clients(id).await.unwrap();
        assert!(clients.iter().all(|client| client.key != bobs_key));
        let kept = clients.iter().find(|client| client.key == cli).unwrap();
        assert_eq!(kept.owner, ClientOwner::Kept);
        assert!(bob.phones().await.unwrap().is_empty());
        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn wifi_reboot_and_play_are_an_owners_and_a_refusal_keeps_the_feeders_reason() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let (owner, id) = adopted(&server, &sim).await;
        let bob = phone(&server, &owner, "bob", Role::Member).await;

        let status = owner.feeder(id).await.unwrap().status.unwrap().status;
        let base = status.wifi_generation;
        refused_for(
            "a member's Wi-Fi change",
            bob.put_wifi(id, base, &home(), key(1)).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        refused_for(
            "a member's restart",
            bob.reboot(id, key(2)).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        let short = WifiChange {
            wifi: Some(WifiCredentials {
                ssid: "home".into(),
                psk: "short".into(),
            }),
        };
        refused_for(
            "a short passphrase",
            owner.put_wifi(id, base, &short, key(3)).await,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        );

        owner.put_wifi(id, base, &home(), key(4)).await.unwrap();
        refused_for(
            "a stale base",
            owner.put_wifi(id, base, &home(), key(5)).await,
            StatusCode::CONFLICT,
            ErrorCode::FeederRefused,
        );
        // The loop does not step the speaker: the feeder's reason, for a
        // member as for an owner.
        refused_for(
            "a tune",
            bob.play(id, TuneName::Chime, key(6)).await,
            StatusCode::CONFLICT,
            ErrorCode::FeederRefused,
        );
        owner.reboot(id, key(7)).await.unwrap();
        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn a_removed_feeder_is_archived_refused_writes_and_adopted_again() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let (owner, id) = adopted(&server, &sim).await;
        let bob = phone(&server, &owner, "bob", Role::Member).await;

        refused(
            bob.remove_feeder(id).await,
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
        );
        let removal = owner.remove_feeder(id).await.unwrap();
        assert_eq!(removal.key, KeyRemoval::Revoked);
        assert!(removal.feeder.archived.is_some());

        refused(
            owner.reboot(id, key(1)).await,
            StatusCode::CONFLICT,
            ErrorCode::FeederArchived,
        );
        refused(
            owner.remove_feeder(id).await,
            StatusCode::CONFLICT,
            ErrorCode::FeederArchived,
        );
        let listed = owner.feeders().await.unwrap();
        assert!(listed.iter().any(|feeder| feeder.id == id));

        let again = owner
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Owner).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap();
        assert_eq!((again.id, again.archived), (id, None));
        server.stop().await;
        sim.stop().await;
    });
}
