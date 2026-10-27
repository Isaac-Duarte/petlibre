//! Adopting a feeder by invite, the connect sequence, and a feeder that
//! no longer knows the server.

use wellfed_api::{AdoptFeeder, ClientRole, Connection as Link, EventBody, EventQuery, User};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, invite, open_owner};
use wellfed_server_testkit::fixed::NOW_MS;
use wellfed_server_testkit::{TestService, local, owner_user};
use wellfed_service::{Error, Invalid};

/// The household user a write comes from.
fn ada() -> User {
    owner_user(1, "ada")
}

#[test]
fn an_adopted_feeder_connects_syncs_its_clock_and_mirrors_its_history() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;

        let detail = server.service().feeder(id).await.unwrap();
        let status = detail.status.unwrap();
        assert!(status.live);
        assert_eq!(
            status
                .status
                .clock
                .map(|clock| clock.as_millisecond() / 1000),
            Some(NOW_MS / 1000),
            "the server set the feeder's clock on connect"
        );

        // Boot, the server's enrolment, the clock sync.
        server.synced(id, status.status.last_event.unwrap()).await;
        let history = server.bodies(id).await;
        assert!(matches!(history[0], EventBody::Boot { .. }));
        assert!(history.iter().any(|body| matches!(
            body,
            EventBody::Enrolled {
                role: ClientRole::Server,
                ..
            }
        )));
        assert!(
            history
                .iter()
                .any(|body| matches!(body, EventBody::ClockSync { .. }))
        );

        let again = server.service().adopt(AdoptFeeder {
            invite: invite(&sim, ClientRole::Server).await,
            name: "again".into(),
            address: Some(sim.address.to_string()),
        });
        assert!(matches!(again.await, Err(Error::AlreadyAdopted)));
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn a_revoked_server_key_is_refused() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let id = server.adopt(&sim).await;
        server.link(id, Link::Online).await;

        let mut owner = open_owner(&sim).await;
        let answer = owner.ask([1; 16], &Op::ListClients).await.unwrap();
        let Answer::Clients(clients) = answer else {
            panic!("{answer:?}");
        };
        let key = clients
            .iter()
            .find(|client| client.role == ClientRole::Server)
            .unwrap()
            .key
            .0;
        let answer = owner.ask([2; 16], &Op::RevokeClient { key }).await.unwrap();
        assert_eq!(answer, Answer::Done { generation: None });

        server.link(id, Link::Refused).await;
        assert!(matches!(
            server
                .service()
                .set_lock(&ada(), id, true, Some([3; 16]))
                .await,
            Err(Error::Offline)
        ));
        server.shutdown().await;
        sim.stop().await;
    });
}

#[test]
fn an_invite_for_a_member_is_adopted_with_that_role_and_bad_input_is_refused() {
    local(async {
        let sim = feeder::start().await;
        let server = TestService::start().await;
        let service = server.service();

        let member = service
            .adopt(AdoptFeeder {
                invite: invite(&sim, ClientRole::Member).await,
                name: "hall".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap();
        assert_eq!(member.role, ClientRole::Member);

        let used = invite(&sim, ClientRole::Server).await;
        let request = |invite: &str, address: Option<&str>| AdoptFeeder {
            invite: invite.into(),
            name: "x".into(),
            address: address.map(Into::into),
        };
        assert!(matches!(
            service.adopt(request("nonsense", Some("127.0.0.1"))).await,
            Err(Error::Invite(_))
        ));
        assert!(matches!(
            service.adopt(request(&used, None)).await,
            Err(Error::AlreadyAdopted)
        ));
        let other = feeder::start().await;
        let fresh = invite(&other, ClientRole::Server).await;
        assert!(matches!(
            service.adopt(request(&fresh, None)).await,
            Err(Error::Invalid(Invalid::AddressRequired))
        ));
        assert!(matches!(
            service.adopt(request(&fresh, Some("not an address"))).await,
            Err(Error::Invalid(Invalid::Address))
        ));
        let query = EventQuery {
            limit: Some(0),
            ..EventQuery::default()
        };
        assert!(matches!(
            service.events(member.id, query).await,
            Err(Error::Invalid(Invalid::Limit))
        ));
        server.shutdown().await;
        other.stop().await;
        sim.stop().await;
    });
}
