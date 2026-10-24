//! Loopback HTTP API of `wellfed-local serve`.
//!
//! Every route except `/health` needs the bearer token from the private
//! configuration, compared in constant time, and write bodies are capped at
//! 4096 bytes. Handlers never touch device state themselves: they hand a
//! `Request` to the MQTT actor and wait up to 5 s. A timeout leaves the outcome
//! unknown, so the caller queries status or repeats the same request ID
//! instead of creating a new one.
//!
//! Response structs declare their fields in sorted order, the order the
//! `json!` bodies they replaced had.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    build_info::BUILD,
    controller::{Rejection, Status},
    mqtt::Request,
    ota,
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

/// A refused request: its status and the `{"error": ..}` body.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: &self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<Rejection> for ApiError {
    fn from(error: Rejection) -> Self {
        let status = match error {
            Rejection::Offline => StatusCode::SERVICE_UNAVAILABLE,
            Rejection::InvalidOta | Rejection::ResolutionEvidence => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Rejection::ImageServer => StatusCode::PRECONDITION_FAILED,
            Rejection::OtaDisabled => StatusCode::FORBIDDEN,
            Rejection::Storage => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::CONFLICT,
        };
        Self::new(status, error.to_string())
    }
}

fn authenticate(headers: &HeaderMap, state: &ApiState) -> Result<(), ApiError> {
    let supplied = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");

    if bool::from(supplied.as_bytes().ct_eq(state.token.as_bytes())) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bearer token required",
        ))
    }
}

async fn operation<T: Serialize>(
    state: ApiState,
    build: impl FnOnce(oneshot::Sender<Result<T, Rejection>>) -> Request,
) -> Result<Json<T>, ApiError> {
    let (send, receive) = oneshot::channel();
    state
        .requests
        .try_send(build(send))
        .map_err(|_| Rejection::Offline)?;

    let result = tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "outcome unknown; query status or repeat the SAME request ID",
            )
        })?
        .map_err(|_| Rejection::Offline)?;

    Ok(Json(result?))
}

async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Status>, ApiError> {
    authenticate(&headers, &state)?;
    Ok(Json(state.status.borrow().clone()))
}

async fn ota_trial_request(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota::trial::Input>,
) -> Result<Json<ota::trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    let lease = ota::image::Lease::current(&state.config_path, state.image_host).await;
    operation(state, |response| Request::OtaTrial(input, lease, response)).await
}

async fn ota_trial_resolve(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota::trial::ResolveInput>,
) -> Result<Json<ota::trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    operation(state, |response| Request::ResolveTrial(input, response)).await
}

/// `/health`: which build is serving, without authentication.
#[derive(Debug, PartialEq, Serialize)]
struct ServiceInfo {
    build_clean: bool,
    build_commit: &'static str,
    service: &'static str,
    version: &'static str,
}

async fn service_info() -> Json<ServiceInfo> {
    Json(ServiceInfo {
        build_clean: BUILD.clean,
        build_commit: BUILD.commit,
        service: "wellfed-local",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(service_info))
        .route("/api/status", get(status))
        .route("/api/ota/trial", post(ota_trial_request))
        .route("/api/ota/trial/resolve", post(ota_trial_resolve))
        .route("/api/replacement/ota/offer", post(replacement_ota_offer))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

/// `/api/replacement/ota/offer`'s body.
#[derive(Debug, Deserialize, Serialize)]
pub struct OfferInput {
    /// Both relative paths under `private/`; absent to clear the offer.
    pub manifest: Option<String>,
    pub image: Option<String>,
}

/// The offer now loaded, or `None` once cleared.
#[derive(Debug, PartialEq, Serialize)]
struct Offered {
    offer: Option<OfferSummary>,
}

#[derive(Debug, PartialEq, Serialize)]
struct OfferSummary {
    bytes: usize,
    version: u32,
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
) -> Result<Json<Offered>, ApiError> {
    authenticate(&headers, &state)?;
    let bad = |message: String| ApiError::new(StatusCode::BAD_REQUEST, message);

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
                crate::ota::firmware::Offer::new(&read(&manifest)?, &read(&image)?)
                    .map_err(|e| bad(e.to_string()))?,
            )
        }
        _ => {
            return Err(bad(
                "give both manifest and image, or neither to clear".into()
            ));
        }
    };

    let summary = offer.as_ref().map(|o| OfferSummary {
        bytes: o.payload.len(),
        version: o.version,
    });

    let (send, receive) = oneshot::channel();
    let unavailable = || ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "server busy");

    state
        .requests
        .try_send(Request::OtaOffer(offer, send))
        .map_err(|_| unavailable())?;

    let configured = tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;

    if !configured {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no replacement-firmware role configured",
        ));
    }

    Ok(Json(Offered { offer: summary }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(value: &impl Serialize) -> String {
        serde_json::to_string(value).unwrap()
    }

    /// The bodies the `json!` replies wrote, byte for byte.
    #[test]
    fn bodies_keep_their_sorted_keys() {
        assert_eq!(
            body(&ServiceInfo {
                build_clean: true,
                build_commit: "abc",
                service: "wellfed-local",
                version: "0.1.0",
            }),
            r#"{"build_clean":true,"build_commit":"abc","service":"wellfed-local","version":"0.1.0"}"#
        );
        assert_eq!(
            body(&Offered {
                offer: Some(OfferSummary {
                    bytes: 4096,
                    version: 7,
                }),
            }),
            r#"{"offer":{"bytes":4096,"version":7}}"#
        );
        assert_eq!(body(&Offered { offer: None }), r#"{"offer":null}"#);
        assert_eq!(body(&ErrorBody { error: "x" }), r#"{"error":"x"}"#);
    }
}
