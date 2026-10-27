//! The handlers, one module per resource as `wellfed-api` lays them out.
//! Each parses, authorizes, calls one service method and maps the result;
//! the OpenAPI document comes from their annotations.

pub(crate) mod auth;
pub(crate) mod events;
pub(crate) mod feeders;
pub(crate) mod stream;

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum_extra::extract::WithRejection;
use wellfed_api::{FeederId, Health};

use crate::error::ApiError;

pub(crate) const TAG_AUTH: &str = "auth";
pub(crate) const TAG_FEEDERS: &str = "feeders";

type ApiResult<T> = Result<T, ApiError>;
type JsonBody<T> = WithRejection<Json<T>, ApiError>;
type IdPath = WithRejection<Path<FeederId>, ApiError>;

/// Liveness and version, without authentication.
#[utoipa::path(get, path = "/health", responses((status = 200, body = Health)))]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

fn insert(response: &mut Response, name: header::HeaderName, value: &str) {
    // Every value here is built from hex, digits and fixed text.
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}
