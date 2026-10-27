//! Health, first-owner setup and the session, over HTTP. No feeder is
//! involved; `end_to_end.rs` adds one.

use wellfed_api::{Credentials, ErrorCode, Role, SetupState};
use wellfed_server_client::reqwest::Response;
use wellfed_server_client::reqwest::header::SET_COOKIE;
use wellfed_server_client::{Auth, Method, StatusCode, expect_status};
use wellfed_server_testkit::{TestServer, ada_login, refused};

/// The session cookie's value from a login's answer, which must keep it
/// from scripts and other sites.
fn cookie(response: &Response) -> String {
    let set = response.headers()[SET_COOKIE].to_str().unwrap();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict"),
        "{set}"
    );
    let pair = set.split(';').next().unwrap();
    pair.split_once('=').unwrap().1.to_owned()
}

#[tokio::test]
async fn health_needs_no_login() {
    let server = TestServer::start().await;
    let health = server.client().health().await.unwrap();
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    server.stop().await;
}

#[tokio::test]
async fn the_first_visitor_becomes_owner_and_setup_closes() {
    let server = TestServer::start().await;
    let client = server.client();
    assert_eq!(
        client.setup_state().await.unwrap(),
        SetupState { open: true }
    );

    let response = client
        .request(Method::POST, "/api/v1/setup")
        .json(&ada_login())
        .send()
        .await
        .unwrap();
    let response = expect_status(response, StatusCode::CREATED).await.unwrap();
    let session = cookie(&response);
    let user: wellfed_api::User = response.json().await.unwrap();
    assert_eq!((user.name.as_str(), user.role), ("ada", Role::Owner));

    assert_eq!(
        client.setup_state().await.unwrap(),
        SetupState { open: false }
    );
    let mut client = client.with_auth(Some(Auth::Session(session)));
    refused(
        server.client().setup(&ada_login()).await,
        StatusCode::CONFLICT,
        ErrorCode::SetupClosed,
    );
    assert_eq!(client.session().await.unwrap(), user);

    // The typed client holds the session a setup or login opens.
    client.logout().await.unwrap();
    assert_eq!(client.auth(), None);
    assert_eq!(client.login(&ada_login()).await.unwrap(), user);
    assert!(matches!(client.auth(), Some(Auth::Session(_))));
    assert_eq!(client.session().await.unwrap(), user);
    server.stop().await;
}

#[tokio::test]
async fn login_and_logout_open_and_end_a_session() {
    let server = TestServer::start().await;
    server.client().setup(&ada_login()).await.unwrap();

    let anonymous = server.client();
    refused(
        anonymous.session().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    let wrong = Credentials {
        password: "wrong horse".into(),
        ..ada_login()
    };
    refused(
        anonymous.clone().login(&wrong).await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::BadCredentials,
    );

    let response = anonymous
        .request(Method::POST, "/api/v1/session")
        .json(&ada_login())
        .send()
        .await
        .unwrap();
    let response = expect_status(response, StatusCode::OK).await.unwrap();
    let client = server
        .client()
        .with_auth(Some(Auth::Session(cookie(&response))));
    client.feeders().await.unwrap();

    let response = client
        .request(Method::DELETE, "/api/v1/session")
        .send()
        .await
        .unwrap();
    let response = expect_status(response, StatusCode::NO_CONTENT)
        .await
        .unwrap();
    assert!(
        response.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    refused(
        client.feeders().await,
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthenticated,
    );
    server.stop().await;
}
