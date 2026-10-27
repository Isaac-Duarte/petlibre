//! Liveness for anyone, readiness for an owner, and the request id every
//! answer carries.

use wellfed_api::{ClientRole, Connection, Credentials, ErrorCode, FeederLink, Role, StoreState};
use wellfed_server_client::reqwest::Response;
use wellfed_server_client::reqwest::header::HOST;
use wellfed_server_client::{Method, StatusCode};
use wellfed_server_testkit::fixed::FixedClock;
use wellfed_server_testkit::{TempStore, TestServer, ada_login, config, feeder, local, refused};
use uuid::Uuid;

const REQUEST_ID: &str = "x-request-id";

fn bob_login() -> Credentials {
    Credentials {
        name: "bob".into(),
        password: "battery staple".into(),
    }
}

/// A server whose one account is bob, a member.
async fn with_a_member() -> TestServer {
    TestServer::open(TempStore::with_member(&bob_login()).await, config()).await
}

/// The answer's request id.
fn request_id(response: &Response) -> String {
    let id = response.headers().get(REQUEST_ID).expect("a request id");
    id.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn health_needs_no_login() {
    let server = TestServer::start().await;
    let health = server.client().health().await.unwrap();
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    server.stop().await;
}

#[tokio::test]
async fn readiness_is_for_an_owner_only() {
    let server = TestServer::start().await;
    refused(
        server.client().status().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    let mut owner = server.client();
    owner.setup(&ada_login()).await.unwrap();
    owner.status().await.unwrap();
    server.stop().await;

    let server = with_a_member().await;
    let mut member = server.client();
    let bob = member.login(&bob_login()).await.unwrap();
    assert_eq!(bob.role, Role::Member);
    refused(
        member.status().await,
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
    );
    member.feeders().await.unwrap();
    server.stop().await;
}

#[test]
fn readiness_shows_the_store_each_feeders_link_and_the_jobs() {
    local(async {
        let sim = feeder::start().await;
        let server = TestServer::start().await;
        let mut owner = server.client();
        owner.setup(&ada_login()).await.unwrap();

        let empty = owner.status().await.unwrap();
        assert_eq!(empty.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(empty.started, FixedClock.now());
        assert_eq!(empty.store, StoreState::Ok);
        assert!(empty.feeders.is_empty());
        assert!(empty.jobs.is_empty(), "no background job runs yet");

        let kitchen = owner
            .adopt(&wellfed_api::AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .unwrap()
            .id;
        wellfed_server_testkit::until("the feeder shows online", || async {
            owner.status().await.unwrap().feeders
                == [FeederLink {
                    id: kitchen,
                    name: "kitchen".into(),
                    connection: Connection::Online,
                }]
        })
        .await;

        // The link follows the session: with the feeder gone, it leaves
        // `online`.
        sim.stop().await;
        wellfed_server_testkit::until("the feeder shows offline", || async {
            owner.status().await.unwrap().feeders[0].connection != Connection::Online
        })
        .await;
        server.stop().await;
    });
}

#[tokio::test]
async fn every_answer_carries_a_request_id() {
    let server = TestServer::start().await;
    let client = server.client();

    let first = client.request(Method::GET, "/health").send().await.unwrap();
    let second = client.request(Method::GET, "/health").send().await.unwrap();
    let first = request_id(&first);
    Uuid::parse_str(&first).expect("a fresh id is a UUID");
    assert_ne!(first, request_id(&second), "each request has its own");

    // A refusal carries one too: the session check's, and the host
    // check's, which runs before any route.
    let unauthenticated = client
        .request(Method::GET, "/api/v1/status")
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    request_id(&unauthenticated);
    let unknown_host = client
        .http()
        .get(client.url("/health"))
        .header(HOST, "rebound.example")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_host.status(), StatusCode::MISDIRECTED_REQUEST);
    request_id(&unknown_host);

    // An id the request brings (a proxy's) is kept, so both logs share it.
    let proxied = client
        .request(Method::GET, "/health")
        .header(REQUEST_ID, "proxy-7f3a")
        .send()
        .await
        .unwrap();
    assert_eq!(request_id(&proxied), "proxy-7f3a");
    server.stop().await;
}

/// An id the request brings is kept only when it is short and plain; any
/// other is replaced with a fresh one, so a client cannot fill or garble
/// the log with it.
#[tokio::test]
async fn a_long_or_odd_request_id_is_replaced() {
    let server = TestServer::start().await;
    let client = server.client();
    let longest = "a".repeat(64);

    for kept in ["proxy.7f3a_B-9", longest.as_str()] {
        let response = client
            .request(Method::GET, "/health")
            .header(REQUEST_ID, kept)
            .send()
            .await
            .unwrap();
        assert_eq!(request_id(&response), kept);
    }

    let too_long = "a".repeat(65);
    for replaced in [too_long.as_str(), "two words", "a/b", "id=\"x\"", ""] {
        let response = client
            .request(Method::GET, "/health")
            .header(REQUEST_ID, replaced)
            .send()
            .await
            .unwrap();
        let id = request_id(&response);
        assert!(Uuid::parse_str(&id).is_ok(), "{replaced:?} became {id:?}");
    }
    server.stop().await;
}
