//! The handlers, one module per resource as `wellfed-api` lays them out.
//! Each parses, authorizes, calls one service method and maps the result;
//! the OpenAPI document comes from their annotations.

pub(crate) mod audit;
pub(crate) mod auth;
pub(crate) mod backups;
pub(crate) mod commands;
pub(crate) mod diagnostics;
pub(crate) mod events;
pub(crate) mod feeders;
pub(crate) mod feeds;
pub(crate) mod foods;
pub(crate) mod health;
pub(crate) mod household;
pub(crate) mod installs;
pub(crate) mod pets;
pub(crate) mod releases;
pub(crate) mod samples;
pub(crate) mod stock;
pub(crate) mod stream;
pub(crate) mod tokens;
pub(crate) mod users;

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
pub(crate) const TAG_COMMANDS: &str = "commands";
pub(crate) const TAG_AUDIT: &str = "audit";
pub(crate) const TAG_HOUSEHOLD: &str = "household";
pub(crate) const TAG_USERS: &str = "users";
pub(crate) const TAG_RELEASES: &str = "releases";
pub(crate) const TAG_TOKENS: &str = "tokens";
pub(crate) const TAG_PETS: &str = "pets";
pub(crate) const TAG_FOODS: &str = "foods";
pub(crate) const TAG_BACKUPS: &str = "backups";

type ApiResult<T> = Result<T, ApiError>;
type JsonBody<T> = WithRejection<Json<T>, ApiError>;
type IdPath = WithRejection<Path<FeederId>, ApiError>;

fn insert(response: &mut Response, name: header::HeaderName, value: &str) {
    // Every value here is built from hex, digits and fixed text.
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}
