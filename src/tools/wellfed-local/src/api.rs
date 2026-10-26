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
use serde::Serialize;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, watch};

use wellfed_stock::{controller::Rejection, ota};

use wellfed_stock::{controller::Status, mqtt::Request};

use crate::build_info::BUILD;

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
    let lease = ota::image::current(&state.config_path, state.image_host).await;
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

/// The operator's client saw the trial's build answer the device API from
/// bank 0 after the trial; it names that build and bank, and its note says
/// what answered. Sends the device nothing.
async fn ota_trial_answered(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota::trial::AnsweredInput>,
) -> Result<Json<ota::trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    operation(state, |response| Request::TrialAnswered(input, response)).await
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
        .route("/api/ota/trial/answered", post(ota_trial_answered))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
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
        assert_eq!(body(&ErrorBody { error: "x" }), r#"{"error":"x"}"#);
    }
}
