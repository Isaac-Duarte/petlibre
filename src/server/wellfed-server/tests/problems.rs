//! How a malformed request comes back: as a typed problem, never a bare
//! status. Most of these requests are ones the typed client cannot make,
//! so they go through its escape hatch with the same session and origin.

use wellfed_api::{ErrorCode, EventQuery, FeederId, StreamQuery};
use wellfed_server_client::{Method, StatusCode, expect_status};
use wellfed_server_testkit::{TestServer, ada_login, refused, refused_for};
use serde_json::json;
use uuid::Uuid;

/// A feeder no server has adopted.
const NOBODY: FeederId = FeederId(Uuid::from_u128(0x0192c5d0_0000_7000_8000_000000000000));

#[tokio::test]
async fn bad_requests_come_back_as_typed_problems() {
    let server = TestServer::start().await;
    let mut client = server.client();
    client.setup(&ada_login()).await.unwrap();

    let unknown = format!("/api/v1/feeders/{NOBODY}");
    let cases = [
        (
            Method::GET,
            unknown.clone(),
            None,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        ),
        (
            Method::GET,
            "/api/v1/feeders/nonsense".into(),
            None,
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
        ),
        (
            Method::PATCH,
            format!("{unknown}/settings"),
            Some(json!({ "volume": 5 })),
            StatusCode::PRECONDITION_REQUIRED,
            ErrorCode::PreconditionRequired,
        ),
        (
            Method::GET,
            format!("{unknown}/events?limit=0"),
            None,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        ),
        (
            Method::GET,
            format!("{unknown}/events?kind=nonsense"),
            None,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        ),
        (
            Method::POST,
            "/api/v1/feeders".into(),
            Some(json!({ "invite": "nonsense", "name": "kitchen" })),
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        ),
        (
            Method::POST,
            "/api/v1/feeders".into(),
            Some(json!({ "name": "kitchen" })),
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        ),
        (
            Method::GET,
            "/api/v1/stream?samples=temperature".into(),
            None,
            StatusCode::BAD_REQUEST,
            ErrorCode::Invalid,
        ),
    ];
    for (method, path, body, status, code) in cases {
        let mut request = client.request(method.clone(), &path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let answer = expect_status(response, StatusCode::OK).await;
        refused_for(&format!("{method} {path}"), answer, status, code);
    }

    // The typed calls that can carry the same faults meet the same answers.
    refused(
        client.feeder(NOBODY).await,
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    );
    let empty = EventQuery {
        limit: Some(0),
        ..EventQuery::default()
    };
    refused(
        client.events(NOBODY, &empty).await,
        StatusCode::BAD_REQUEST,
        ErrorCode::Invalid,
    );
    let temperature = StreamQuery {
        samples: Some("temperature".into()),
    };
    refused(
        client.stream(&temperature, None).await,
        StatusCode::BAD_REQUEST,
        ErrorCode::Invalid,
    );

    let response = client
        .request(Method::PATCH, &format!("{unknown}/settings"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    refused(
        expect_status(response, StatusCode::OK).await,
        StatusCode::PRECONDITION_REQUIRED,
        ErrorCode::PreconditionRequired,
    );

    let response = client
        .request(Method::POST, &format!("{unknown}/lock"))
        .header("Idempotency-Key", "7")
        .json(&json!({ "locked": true }))
        .send()
        .await
        .unwrap();
    refused(
        expect_status(response, StatusCode::NO_CONTENT).await,
        StatusCode::BAD_REQUEST,
        ErrorCode::Invalid,
    );
    server.stop().await;
}
