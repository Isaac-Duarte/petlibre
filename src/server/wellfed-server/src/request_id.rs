//! A request id on every request: in the span every log line of the
//! request is written under, and in the answer's `x-request-id`, so an
//! owner reporting a failed request can name the log lines it wrote.
//!
//! A request that already carries `x-request-id` (set by a reverse proxy,
//! so its log and this one share the id) keeps it; any other gets a random
//! UUID. Header values hold no line breaks, so a client's id cannot forge
//! a log line.

use axum::http::{HeaderName, Request};
use tower_http::request_id::RequestId;
use tracing::Span;

/// The header the id travels in, both ways.
pub(crate) const HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The span a request runs in: its id, method and path. The query is left
/// out, so nothing a client puts there reaches the log.
pub(crate) fn span<B>(request: &Request<B>) -> Span {
    let id = request
        .extensions()
        .get::<RequestId>()
        .and_then(|id| id.header_value().to_str().ok())
        .unwrap_or("-");
    tracing::info_span!(
        "request",
        id,
        method = %request.method(),
        path = request.uri().path(),
    )
}
