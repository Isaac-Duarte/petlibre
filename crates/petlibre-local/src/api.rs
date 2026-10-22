//! Loopback HTTP API of `petlibre-local serve`.
//!
//! Every route except `/health` needs the bearer token from the private
//! configuration, compared in constant time, and write bodies are capped at
//! 4096 bytes. Handlers never touch device state themselves: they hand a
//! `Request` to the MQTT actor and wait up to 5 s. A timeout leaves the outcome
//! unknown, so the caller queries status or repeats the same request ID
//! instead of creating a new one.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    build_info::BUILD,
    controller::{FeedInput, FeedRecord, Rejection, ResolveInput, Status},
    mqtt::Request,
    ota_image, ota_trial, probe,
    replacement::{self, RequestError},
};

#[derive(Clone)]
pub struct ApiState {
    pub token: Arc<str>,
    pub requests: mpsc::Sender<Request>,
    pub status: watch::Receiver<Status>,
    /// Config location; the image server's private lease is written beside it.
    pub config_path: Arc<std::path::Path>,
    pub image_host: std::net::IpAddr,
}

type ApiError = (StatusCode, Json<Value>);

fn authenticate(headers: &HeaderMap, state: &ApiState) -> Result<(), ApiError> {
    let supplied = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");

    if bool::from(supplied.as_bytes().ct_eq(state.token.as_bytes())) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"bearer token required"})),
        ))
    }
}

fn reject(error: Rejection) -> ApiError {
    let status = match error {
        Rejection::Offline => StatusCode::SERVICE_UNAVAILABLE,
        Rejection::InvalidPortions => StatusCode::UNPROCESSABLE_ENTITY,
        Rejection::InvalidOta | Rejection::ResolutionEvidence | Rejection::InvalidProbe => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        Rejection::ImageServer => StatusCode::PRECONDITION_FAILED,
        Rejection::OtaDisabled => StatusCode::FORBIDDEN,
        Rejection::Storage => StatusCode::INTERNAL_SERVER_ERROR,
        Rejection::Cooldown => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::CONFLICT,
    };
    (status, Json(json!({"error":error.message()})))
}

async fn operation<T: Serialize>(
    state: ApiState,
    build: impl FnOnce(oneshot::Sender<Result<T, Rejection>>) -> Request,
) -> Result<Json<T>, ApiError> {
    let (send, receive) = oneshot::channel();
    state
        .requests
        .try_send(build(send))
        .map_err(|_| reject(Rejection::Offline))?;

    let result = tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(
                    json!({"error":"outcome unknown; query status or repeat the SAME request ID"}),
                ),
            )
        })?
        .map_err(|_| reject(Rejection::Offline))?;

    result.map(Json).map_err(reject)
}

async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Status>, ApiError> {
    authenticate(&headers, &state)?;
    Ok(Json(state.status.borrow().clone()))
}

async fn feed(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<FeedInput>,
) -> Result<Json<FeedRecord>, ApiError> {
    authenticate(&headers, &state)?;
    operation(state, |response| Request::Feed(input, response)).await
}

async fn resolve(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ResolveInput>,
) -> Result<Json<FeedRecord>, ApiError> {
    authenticate(&headers, &state)?;
    operation(state, |response| Request::Resolve(input, response)).await
}

async fn ota_trial_request(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota_trial::Input>,
) -> Result<Json<ota_trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    let lease = ota_image::Lease::current(&state.config_path, state.image_host).await;
    operation(state, |response| Request::OtaTrial(input, lease, response)).await
}

async fn ota_trial_resolve(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota_trial::ResolveInput>,
) -> Result<Json<ota_trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    operation(state, |response| Request::ResolveTrial(input, response)).await
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async {
                Json(
                    json!({"service":"petlibre-local","version":env!("CARGO_PKG_VERSION"),
                    "build_commit":BUILD.commit,"build_clean":BUILD.clean}),
                )
            }),
        )
        .route("/api/status", get(status))
        .route("/api/feed", post(feed))
        .route("/api/feed/resolve", post(resolve))
        .route("/api/ota/trial", post(ota_trial_request))
        .route("/api/ota/trial/resolve", post(ota_trial_resolve))
        .route("/api/probe/query", post(probe_query))
        .route("/api/replacement/request", post(replacement_request))
        .route("/api/replacement/ota/offer", post(replacement_ota_offer))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

/// One read-only diagnostic request to the replacement firmware. The body is
/// parsed here so any unsupported operation is a plain 400.
async fn replacement_request(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    authenticate(&headers, &state)?;

    let input: replacement::Input = serde_json::from_slice(&body)
        .ok()
        .filter(replacement::Input::valid)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"expected {\"op\":\"health\"|\"ping\"|\"settings\"|\"wifi\"|\"play\"}; settings may set lights, sound (booleans) and volume (0-100); wifi may set ssid (1-32 bytes) with psk (8-63 printable ASCII), or clear: true; play needs tune: chime, meal, coin, alert, test or twinkle"})),
            )
        })?;

    let (send, receive) = oneshot::channel();
    let offline = |error: RequestError| {
        let status = match error {
            RequestError::Unconfigured => StatusCode::NOT_FOUND,
            RequestError::Offline | RequestError::Exhausted => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, Json(json!({"error":error.message()})))
    };

    let op = input.op;
    state
        .requests
        .try_send(Request::Replacement(input, send))
        .map_err(|_| offline(RequestError::Offline))?;
    let id = tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| offline(RequestError::Offline))?
        .map_err(|_| offline(RequestError::Offline))?
        .map_err(offline)?;

    Ok(Json(json!({"id":id,"op":op})))
}

#[derive(serde::Deserialize)]
struct OfferInput {
    /// Both relative paths under `private/`; absent to clear the offer.
    manifest: Option<String>,
    image: Option<String>,
}

fn private_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(name);
    let clean = path.is_relative()
        && path.starts_with("private")
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    clean.then(|| path.to_path_buf())
}

/// Load or clear the replacement-firmware update the server will answer
/// `ota/get` from. The files are read from `private/`; the firmware verifies
/// the signature, the server only checks the image matches its manifest.
async fn replacement_ota_offer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<OfferInput>,
) -> Result<Json<Value>, ApiError> {
    authenticate(&headers, &state)?;
    let bad = |message: String| (StatusCode::BAD_REQUEST, Json(json!({"error": message})));

    let offer = match (input.manifest.as_deref(), input.image.as_deref()) {
        (None, None) => None,
        (Some(manifest), Some(image)) => {
            let (Some(manifest), Some(image)) = (private_path(manifest), private_path(image))
            else {
                return Err(bad("paths must be relative and under private/".into()));
            };
            let read = |p: &std::path::Path| {
                std::fs::read(p).map_err(|e| bad(format!("{}: {e}", p.display())))
            };
            Some(
                crate::firmware_ota::Offer::new(&read(&manifest)?, &read(&image)?)
                    .map_err(|e| bad(e.to_string()))?,
            )
        }
        _ => {
            return Err(bad(
                "give both manifest and image, or neither to clear".into()
            ));
        }
    };

    let summary = offer
        .as_ref()
        .map(|o| json!({"version": o.version, "bytes": o.payload.len()}));

    let (send, receive) = oneshot::channel();
    let unavailable = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"server busy"})),
        )
    };

    state
        .requests
        .try_send(Request::OtaOffer(offer, send))
        .map_err(|_| unavailable())?;

    let configured = tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;

    if !configured {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"no replacement-firmware role configured"})),
        ));
    }

    Ok(Json(json!({"offer": summary})))
}

async fn probe_query(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<probe::Input>,
) -> Result<Json<probe::Record>, ApiError> {
    authenticate(&headers, &state)?;
    operation(state, |response| Request::ProbeQuery(input, response)).await
}
