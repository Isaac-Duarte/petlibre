//! Which requests the server takes at all: one for a host it answers to,
//! and a write from its own origin. These requests leave out what the
//! client sends, so they go through its underlying HTTP client.

use wellfed_api::{ErrorCode, SetupState};
use wellfed_server_client::reqwest::header::{HOST, ORIGIN};
use wellfed_server_client::{StatusCode, decode, expect_status};
use wellfed_server_testkit::{TestServer, ada_login, refused};

/// A DNS-rebinding page sends its own name as `Host` and `Origin`, which
/// match each other; the name is not one the server answers to.
#[tokio::test]
async fn a_request_for_another_host_is_refused() {
    let server = TestServer::start().await;
    let client = server.client();

    let response = client
        .http()
        .post(client.url("/api/v1/setup"))
        .header(HOST, "rebound.example:8080")
        .header(ORIGIN, "http://rebound.example:8080")
        .json(&ada_login())
        .send()
        .await
        .unwrap();
    refused(
        expect_status(response, StatusCode::CREATED).await,
        StatusCode::MISDIRECTED_REQUEST,
        ErrorCode::UnknownHost,
    );

    assert_eq!(
        client.setup_state().await.unwrap(),
        SetupState { open: true },
        "nothing was created"
    );
    server.stop().await;
}

#[tokio::test]
async fn a_write_from_another_origin_is_refused() {
    let server = TestServer::start().await;
    let client = server.client();

    for origin in [None, Some("http://evil.example")] {
        let mut request = client
            .http()
            .post(client.url("/api/v1/setup"))
            .json(&ada_login());
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        refused(
            expect_status(request.send().await.unwrap(), StatusCode::CREATED).await,
            StatusCode::FORBIDDEN,
            ErrorCode::CrossOrigin,
        );
    }

    let read = client
        .http()
        .get(client.url("/api/v1/setup"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        decode::<SetupState>(read, StatusCode::OK).await.unwrap(),
        SetupState { open: true },
        "reads need no origin, and nothing was created"
    );
    server.stop().await;
}
