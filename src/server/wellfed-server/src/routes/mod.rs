//! The handlers, one module per resource as `wellfed-api` lays them out.
//! Each parses, authorizes, calls one service method and maps the result;
//! the OpenAPI document comes from their annotations.

pub(crate) mod auth;
pub(crate) mod events;
pub(crate) mod feeders;
pub(crate) mod health;
pub(crate) mod stream;

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum_extra::extract::WithRejection;
use wellfed_api::FeederId;

use crate::error::ApiError;

pub(crate) const TAG_AUTH: &str = "auth";
pub(crate) const TAG_FEEDERS: &str = "feeders";
pub(crate) const TAG_HEALTH: &str = "health";

type ApiResult<T> = Result<T, ApiError>;
type JsonBody<T> = WithRejection<Json<T>, ApiError>;
type IdPath = WithRejection<Path<FeederId>, ApiError>;

fn insert(response: &mut Response, name: header::HeaderName, value: &str) {
    // Every value here is built from hex, digits and fixed text.
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}
