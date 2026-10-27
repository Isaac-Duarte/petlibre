//! Failed logins over HTTP: limited per name and per address with `429`
//! and `Retry-After`, never locking an account or touching the sessions
//! and tokens already open; and the client's address and scheme believed
//! from a trusted proxy's forwarded headers only.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use wellfed_api::{AuditAction, Credentials, ErrorCode, NewToken, PREFIX, PageQuery};
use wellfed_server_client::reqwest;
use wellfed_server_client::reqwest::header::SET_COOKIE;
use wellfed_server_client::{Auth, Client, Error, Method, StatusCode, expect_status};
use wellfed_server_testkit::fixed::ManualClock;
use wellfed_server_testkit::{TestServer, ada_login, ada_setup, local, refused};

fn wrong(name: &str) -> Credentials {
    Credentials {
        name: name.into(),
        password: "not the password".into(),
    }
}

/// The owner's client, after setup.
async fn owner(server: &TestServer) -> Client {
    let mut client = server.client();
    client.setup(&ada_setup()).await.unwrap();
    client
}

/// A login sent as if a proxy forwarded it from `client`, over `proto`.
async fn forwarded_login(
    server: &TestServer,
    credentials: &Credentials,
    client: &str,
    proto: &str,
) -> reqwest::Response {
    server
        .client()
        .request(Method::POST, &format!("{PREFIX}/session"))
        .header("x-forwarded-for", format!("192.0.2.1, {client}"))
        .header("x-forwarded-proto", proto)
        .json(credentials)
        .send()
        .await
        .unwrap()
}

/// The addresses the audit log saw the owner's logins and failed logins
/// come from, oldest first.
async fn login_addresses(owner: &Client) -> Vec<IpAddr> {
    let log = owner.audit(&PageQuery::default()).await.unwrap();
    log.items
        .iter()
        .filter_map(|entry| match entry.action {
            AuditAction::Login { from } | AuditAction::LoginFailed { from, .. } => Some(from),
            _ => None,
        })
        .collect()
}

/// Five failures for a name, from anywhere, and the sixth is refused
/// before its password is checked, the right one included. Sessions and
/// tokens already open carry on, and the name is taken again as the
/// minute moves on: no account is locked.
#[test]
fn a_name_takes_five_failures_a_minute_and_nothing_else_stops() {
    local(async {
        let clock = ManualClock::new();
        let server = TestServer::start_with_clock(Arc::new(clock.clone())).await;
        let owner = owner(&server).await;
        let phone = NewToken {
            name: "phone".into(),
            expires: None,
        };
        let token = owner.create_token(&phone).await.unwrap();
        let phone = server.client().with_auth(Some(Auth::Token(token.secret)));

        for _ in 0..5 {
            refused(
                server.client().login(&wrong("ada")).await,
                StatusCode::UNAUTHORIZED,
                ErrorCode::BadCredentials,
            );
        }
        let limited = server.client().login(&ada_login()).await;
        assert_eq!(
            limited.as_ref().err().and_then(Error::retry_after),
            Some(Duration::from_secs(12))
        );
        refused(
            limited,
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::LoginLimited,
        );
        refused(
            server.client().login(&wrong("ADA ")).await,
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::LoginLimited,
        );

        owner.session().await.unwrap();
        phone.session().await.unwrap();
        owner.users().await.unwrap();

        clock.advance(Duration::from_secs(12));
        server.client().login(&ada_login()).await.unwrap();
        server.stop().await;
    });
}

/// Without a trusted proxy, every client here is 127.0.0.1, whatever its
/// headers say: twenty failures across names, and the address is refused.
#[test]
fn an_address_takes_twenty_failures_and_forged_headers_are_ignored() {
    local(async {
        let server = TestServer::start().await;
        let owner = owner(&server).await;

        let answer = forwarded_login(&server, &wrong("ada"), "203.0.113.50", "https").await;
        assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
        for n in 0..19 {
            let (name, client) = (format!("name {}", n / 5), format!("203.0.113.{n}"));
            let answer = forwarded_login(&server, &wrong(&name), &client, "https").await;
            assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
        }
        let answer = forwarded_login(&server, &ada_login(), "203.0.113.99", "https").await;
        assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(answer.headers()["retry-after"], "3");

        owner.session().await.unwrap();
        let here: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(login_addresses(&owner).await, [here]);
        server.stop().await;
    });
}

/// The server serves only HTTP, so without a trusted proxy its cookie is
/// never `Secure`, whatever the request claims.
#[test]
fn a_forged_scheme_never_makes_the_cookie_secure() {
    local(async {
        let server = TestServer::start().await;
        owner(&server).await;

        let answer = forwarded_login(&server, &ada_login(), "203.0.113.7", "https").await;

        let answer = expect_status(answer, StatusCode::OK).await.unwrap();
        let cookie = answer.headers()[SET_COOKIE].to_str().unwrap();
        assert!(!cookie.contains("Secure"), "{cookie}");
        server.stop().await;
    });
}

/// Behind a trusted proxy, the client is the forwarded address the proxy
/// appended, and an `https` scheme makes the cookie `Secure`. A client's
/// own entries left of it are ignored.
#[test]
fn a_trusted_proxy_names_the_client_and_its_scheme() {
    local(async {
        let clock = ManualClock::new();
        let server = TestServer::start_behind(Arc::new(clock), &["127.0.0.0/8"]).await;
        let owner = owner(&server).await;

        let answer = forwarded_login(&server, &ada_login(), "203.0.113.7", "https").await;
        let answer = expect_status(answer, StatusCode::OK).await.unwrap();
        let cookie = answer.headers()[SET_COOKIE].to_str().unwrap();
        assert!(cookie.ends_with("; Secure"), "{cookie}");

        let answer = forwarded_login(&server, &ada_login(), "203.0.113.8", "http").await;
        let answer = expect_status(answer, StatusCode::OK).await.unwrap();
        let cookie = answer.headers()[SET_COOKIE].to_str().unwrap();
        assert!(!cookie.contains("Secure"), "{cookie}");

        let answer = forwarded_login(&server, &wrong("ada"), "203.0.113.9", "https").await;
        assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);

        let from: Vec<IpAddr> = ["203.0.113.7", "203.0.113.8", "203.0.113.9"]
            .map(|address| address.parse().unwrap())
            .into();
        assert_eq!(login_addresses(&owner).await, from);
        server.stop().await;
    });
}

/// Behind a trusted proxy each forwarded client has its own address
/// bucket: one client's failures do not limit another's.
#[test]
fn behind_a_trusted_proxy_each_client_is_limited_alone() {
    local(async {
        let clock = ManualClock::new();
        let server = TestServer::start_behind(Arc::new(clock), &["127.0.0.1"]).await;
        owner(&server).await;

        for n in 0..4 {
            let name = format!("name {n}");
            for _ in 0..5 {
                let answer = forwarded_login(&server, &wrong(&name), "203.0.113.1", "https").await;
                assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
            }
        }
        let limited = forwarded_login(&server, &ada_login(), "203.0.113.1", "https").await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

        let other = forwarded_login(&server, &ada_login(), "203.0.113.2", "https").await;
        assert_eq!(other.status(), StatusCode::OK);
        server.stop().await;
    });
}
