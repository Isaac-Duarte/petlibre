//! The HTTP API in process: setup, login, the origin check, and how
//! failures come back. No feeder is involved; `end_to_end.rs` adds one.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use jiff::tz::TimeZone;
use wellfed_service::{Config, Discovery, Service, SystemClock, SystemRandom, Timing};
use wellfed_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const HOST: &str = "server.test";
const ORIGIN: &str = "http://server.test";

struct Api {
    _dir: TempDir,
    router: Router,
}

impl Api {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("server.db")).await.unwrap();
        let config = Config {
            key: [7; 32],
            zone: TimeZone::UTC,
            label: "test-server".into(),
            discovery: Discovery::Off,
            timing: Timing {
                session: Duration::from_secs(600),
                ..Timing::default()
            },
        };
        let service = Service::start(store, config, Arc::new(SystemClock), Arc::new(SystemRandom))
            .await
            .unwrap();
        Self {
            _dir: dir,
            router: wellfed_server::app(
                service,
                CancellationToken::new(),
                wellfed_server::Hosts::new([HOST.to_owned()]),
            ),
        }
    }

    /// One request from this server's own page (with its `Origin`), with
    /// `cookie` when given.
    async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> (StatusCode, HeaderMap, Value) {
        self.send(method, uri, body, cookie, Some(ORIGIN)).await
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        cookie: Option<&str>,
        origin: Option<&str>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, HOST);
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();

        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, headers, value)
    }
}

/// The `name=value` part of a `Set-Cookie` header.
fn cookie(headers: &HeaderMap) -> String {
    let set = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict"),
        "{set}"
    );
    set.split(';').next().unwrap().to_owned()
}

fn ada() -> Value {
    json!({ "name": "ada", "password": "correct horse" })
}

#[tokio::test]
async fn health_needs_no_login() {
    let api = Api::start().await;
    let (status, _, body) = api.call(Method::GET, "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn the_first_visitor_becomes_owner_and_setup_closes() {
    let api = Api::start().await;
    let (_, _, state) = api.call(Method::GET, "/api/v1/setup", None, None).await;
    assert_eq!(state, json!({ "open": true }));

    let (status, headers, user) = api
        .call(Method::POST, "/api/v1/setup", Some(ada()), None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        (user["name"].as_str(), user["role"].as_str()),
        (Some("ada"), Some("owner"))
    );
    let session = cookie(&headers);

    let (_, _, state) = api.call(Method::GET, "/api/v1/setup", None, None).await;
    assert_eq!(state, json!({ "open": false }));
    let (status, headers, problem) = api
        .call(Method::POST, "/api/v1/setup", Some(ada()), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(problem["code"], "setup_closed");
    assert_eq!(problem["status"], 409);

    let (status, _, me) = api
        .call(Method::GET, "/api/v1/session", None, Some(&session))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me, user);
}

#[tokio::test]
async fn login_and_logout_open_and_end_a_session() {
    let api = Api::start().await;
    api.call(Method::POST, "/api/v1/setup", Some(ada()), None)
        .await;

    let (status, _, problem) = api.call(Method::GET, "/api/v1/session", None, None).await;
    assert_eq!(
        (status, problem["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("unauthenticated"))
    );
    let wrong = json!({ "name": "ada", "password": "wrong horse" });
    let (status, _, problem) = api
        .call(Method::POST, "/api/v1/session", Some(wrong), None)
        .await;
    assert_eq!(
        (status, problem["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("bad_credentials"))
    );

    let (status, headers, _) = api
        .call(Method::POST, "/api/v1/session", Some(ada()), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let session = cookie(&headers);
    let (status, _, _) = api
        .call(Method::GET, "/api/v1/feeders", None, Some(&session))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, headers, _) = api
        .call(Method::DELETE, "/api/v1/session", None, Some(&session))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let (status, _, _) = api
        .call(Method::GET, "/api/v1/feeders", None, Some(&session))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A DNS-rebinding page sends its own name as `Host` and `Origin`, which
/// match each other; the name is not one the server answers to.
#[tokio::test]
async fn a_request_for_another_host_is_refused() {
    let api = Api::start().await;
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/setup")
        .header(header::HOST, "rebound.example:8080")
        .header(header::ORIGIN, "http://rebound.example:8080")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(ada().to_string()))
        .unwrap();
    let response = api.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(problem["code"], "unknown_host");

    let (_, _, state) = api.call(Method::GET, "/api/v1/setup", None, None).await;
    assert_eq!(state, json!({ "open": true }), "nothing was created");
}

#[tokio::test]
async fn a_write_from_another_origin_is_refused() {
    let api = Api::start().await;
    for origin in [None, Some("http://evil.example")] {
        let (status, _, problem) = api
            .send(Method::POST, "/api/v1/setup", Some(ada()), None, origin)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin:?}");
        assert_eq!(problem["code"], "cross_origin");
    }
    let (_, _, state) = api
        .send(Method::GET, "/api/v1/setup", None, None, None)
        .await;
    assert_eq!(
        state,
        json!({ "open": true }),
        "reads need no origin, and nothing was created"
    );
}

#[tokio::test]
async fn bad_requests_come_back_as_typed_problems() {
    let api = Api::start().await;
    let (_, headers, _) = api
        .call(Method::POST, "/api/v1/setup", Some(ada()), None)
        .await;
    let session = cookie(&headers);
    let me = Some(session.as_str());
    let unknown = "/api/v1/feeders/0192c5d0-0000-7000-8000-000000000000";

    let cases = [
        (
            Method::GET,
            unknown.to_owned(),
            None,
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            Method::GET,
            "/api/v1/feeders/nonsense".into(),
            None,
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            Method::PATCH,
            format!("{unknown}/settings"),
            Some(json!({ "volume": 5 })),
            StatusCode::PRECONDITION_REQUIRED,
            "precondition_required",
        ),
        (
            Method::GET,
            format!("{unknown}/events?limit=0"),
            None,
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
        (
            Method::GET,
            format!("{unknown}/events?kind=nonsense"),
            None,
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
        (
            Method::POST,
            "/api/v1/feeders".into(),
            Some(json!({ "invite": "nonsense", "name": "kitchen" })),
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
        (
            Method::POST,
            "/api/v1/feeders".into(),
            Some(json!({ "name": "kitchen" })),
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
        (
            Method::GET,
            "/api/v1/stream?samples=temperature".into(),
            None,
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
    ];
    for (method, uri, body, status, code) in cases {
        let (got, headers, problem) = api.call(method.clone(), &uri, body, me).await;
        assert_eq!(
            (got, problem["code"].as_str()),
            (status, Some(code)),
            "{method} {uri}"
        );
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    }

    let (status, _, problem) = api
        .call(
            Method::PATCH,
            &format!("{unknown}/settings"),
            Some(json!({})),
            me,
        )
        .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{problem}");
}
