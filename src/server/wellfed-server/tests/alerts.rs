//! Alerts over HTTP, against the simulator and a clock the test moves: a
//! feeder gone past ten minutes raises an alert everyone reads and the
//! stream carries, an owner acknowledges it, and the feeder coming back
//! clears it.

use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{
    AdoptFeeder, Alert, AlertCondition, AlertId, AlertQuery, AlertState, ClientRole, Connection,
    Credentials, ErrorCode, NewUser, Role, StreamEvent, StreamQuery,
};
use wellfed_server_client::{Client, StatusCode, Updates};
use wellfed_server_testkit::feeder::{self, Flash};
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestServer, WAIT, ada_setup, local, refused, until};
use uuid::Uuid;

const PASSWORD: &str = "temporary pass";

async fn account(server: &TestServer, owner: &Client, name: &str, role: Role) -> Client {
    let new = NewUser {
        name: name.into(),
        password: PASSWORD.into(),
        role,
        expires: None,
    };
    owner.create_user(&new).await.unwrap();
    let mut client = server.client();
    client
        .login(&Credentials {
            name: name.into(),
            password: PASSWORD.into(),
        })
        .await
        .unwrap();
    client
}

async fn connection(http: &Client) -> Connection {
    http.feeders().await.unwrap()[0].connection
}

/// The next alert update on the stream.
async fn next_alert(updates: &mut Updates) -> Alert {
    loop {
        let update = tokio::time::timeout(WAIT, updates.next())
            .await
            .expect("an update in time")
            .expect("the stream is open")
            .unwrap();
        if let StreamEvent::Alert { alert } = update.event {
            return alert;
        }
    }
}

fn open() -> AlertQuery {
    AlertQuery {
        state: Some(AlertState::Open),
        ..AlertQuery::default()
    }
}

#[test]
fn an_offline_feeder_raises_an_alert_an_owner_acknowledges_and_its_return_clears() {
    local(async {
        let clock = ManualClock::new();
        let flash = Flash::memory();
        let sim = feeder::start_on(&flash).await;
        let address = sim.address;
        let server = TestServer::start_with_clock(Arc::new(clock.clone())).await;
        let mut owner = server.client();
        owner.setup(&ada_setup()).await.unwrap();
        let member = account(&server, &owner, "bob", Role::Member).await;
        let viewer = account(&server, &owner, "vic", Role::Viewer).await;
        let feeder = owner
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(address.to_string()),
            })
            .await
            .unwrap()
            .id;
        until("the server connects", || async {
            connection(&owner).await == Connection::Online
        })
        .await;
        let mut updates = viewer.stream(&StreamQuery::default(), None).await.unwrap();

        sim.stop().await;
        until("the server sees the feeder go", || async {
            connection(&owner).await != Connection::Online
        })
        .await;
        clock.advance(Duration::from_secs(5 * 60));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            owner.alerts(&open()).await.unwrap().items.is_empty(),
            "not past ten minutes"
        );

        clock.advance(Duration::from_secs(6 * 60));
        let raised = next_alert(&mut updates).await;
        assert_eq!(
            (raised.feeder, raised.condition, raised.state),
            (feeder, AlertCondition::Offline, AlertState::Open)
        );
        for (who, client) in [("member", &member), ("viewer", &viewer)] {
            let page = client.alerts(&AlertQuery::default()).await.unwrap();
            assert_eq!(page.items, [raised], "{who}");
            assert_eq!(page.next, None);
        }

        for client in [&member, &viewer] {
            let refusal = client.acknowledge_alert(raised.id).await;
            refused(refusal, StatusCode::FORBIDDEN, ErrorCode::Forbidden);
        }
        let unknown = AlertId(Uuid::from_u128(1));
        let missing = owner.acknowledge_alert(unknown).await;
        refused(missing, StatusCode::NOT_FOUND, ErrorCode::NotFound);
        let acked = owner.acknowledge_alert(raised.id).await.unwrap();
        assert_eq!(acked.state, AlertState::Acknowledged);
        assert!(acked.acknowledged.is_some());
        assert_eq!(next_alert(&mut updates).await, acked);
        let again = owner.acknowledge_alert(raised.id).await.unwrap();
        assert_eq!(again, acked, "a second acknowledgement changes nothing");
        assert!(owner.alerts(&open()).await.unwrap().items.is_empty());

        let sim = feeder::restart(&flash, address).await;
        let cleared = next_alert(&mut updates).await;
        assert_eq!(cleared.id, raised.id);
        assert_eq!(cleared.state, AlertState::Cleared);
        assert_eq!(cleared.acknowledged, acked.acknowledged, "kept");
        let gone = AlertQuery {
            state: Some(AlertState::Cleared),
            feeder: Some(feeder),
            ..AlertQuery::default()
        };
        assert_eq!(owner.alerts(&gone).await.unwrap().items, [cleared]);

        server.stop().await;
        sim.stop().await;
    });
}

#[test]
fn a_page_of_alerts_is_bounded_and_its_cursor_checked() {
    local(async {
        let server = TestServer::start().await;
        let mut owner = server.client();
        owner.setup(&ada_setup()).await.unwrap();

        let empty = owner.alerts(&AlertQuery::default()).await.unwrap();
        assert!(empty.items.is_empty());
        assert_eq!(empty.next, None);
        for query in [
            AlertQuery {
                limit: Some(0),
                ..AlertQuery::default()
            },
            AlertQuery {
                after: Some("x".into()),
                ..AlertQuery::default()
            },
        ] {
            let bad = owner.alerts(&query).await;
            refused(bad, StatusCode::BAD_REQUEST, ErrorCode::Invalid);
        }
        let anonymous = server.client().alerts(&AlertQuery::default()).await;
        refused(
            anonymous,
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
        );
        server.stop().await;
    });
}
