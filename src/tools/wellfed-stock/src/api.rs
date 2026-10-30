//! Loopback HTTP API of the stock server ([`crate::server`]).
//!
//! Every route except `/health` needs the bearer token from the private
//! configuration, compared in constant time, and write bodies are capped at
//! 4096 bytes. Handlers never touch device state themselves: they go
//! through the server's [`Handle`], which hands a `Request` to the MQTT
//! actor and waits up to 5 s. A timeout leaves the outcome unknown, so the
//! caller queries status or repeats the same request ID instead of creating
//! a new one.
//!
//! Response structs declare their fields in sorted order, the order the
//! `json!` bodies they replaced had.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use subtle::ConstantTimeEq;

use crate::{
    controller::{Rejection, Status},
    ota,
    server::{Handle, Refusal},
};

use crate::build_info::BUILD;

/// The status's whole answer when the enrollment record cannot be read.
pub const ENROLLMENT_UNREADABLE: &str = "enrollment unreadable";

#[derive(Clone)]
pub struct ApiState {
    pub token: Arc<str>,
    pub handle: Handle,
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

impl From<Refusal> for ApiError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Contested => Self::new(StatusCode::CONFLICT, refusal.to_string()),
            Refusal::Rejected(rejection) => rejection.into(),
            Refusal::Unanswered => Self::new(StatusCode::GATEWAY_TIMEOUT, refusal.to_string()),
        }
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

async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Status>, ApiError> {
    authenticate(&headers, &state)?;
    // The record holds the feeder's password, and a parse error can quote
    // a field's value, so the details stay in the server's log and the
    // body is fixed.
    let status = state.handle.status().map_err(|error| {
        tracing::warn!("enrollment unreadable: {error:#}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, ENROLLMENT_UNREADABLE)
    })?;
    Ok(Json(status))
}

async fn ota_trial_request(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota::trial::Input>,
) -> Result<Json<ota::trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    Ok(Json(state.handle.trial(input).await?))
}

async fn ota_trial_resolve(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ota::trial::ResolveInput>,
) -> Result<Json<ota::trial::Record>, ApiError> {
    authenticate(&headers, &state)?;
    Ok(Json(state.handle.resolve(input).await?))
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
    Ok(Json(state.handle.answered(input).await?))
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
        service: "wellfed-stock",
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
                service: "wellfed-stock",
                version: "0.1.0",
            }),
            r#"{"build_clean":true,"build_commit":"abc","service":"wellfed-stock","version":"0.1.0"}"#
        );
        assert_eq!(body(&ErrorBody { error: "x" }), r#"{"error":"x"}"#);
    }
}
