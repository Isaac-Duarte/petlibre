//! A request id on every request: in the span every log line of the
//! request is written under, and in the answer's `x-request-id`, so an
//! owner reporting a failed request can name the log lines it wrote.
//!
//! A request that already carries one plain `x-request-id` (set by a
//! reverse proxy, so its log and this one share the id) keeps it. Any other
//! gets a random UUID: so does one whose id is longer than [`LONGEST`] or
//! holds anything but letters, digits, `.`, `_` and `-`, so a client can
//! neither fill the log nor garble it with its id.

use axum::extract::Request;
use axum::http::HeaderName;
use tower_http::request_id::RequestId;
use tracing::Span;

/// The header the id travels in, both ways.
pub(crate) const HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The longest id a request may bring.
const LONGEST: usize = 64;

/// Drop a request's own id unless it is one plain id, so the id layer
/// makes a fresh one.
pub(crate) async fn screen(mut request: Request) -> Request {
    let mut ids = request.headers().get_all(&HEADER).iter();
    let plain = match (ids.next(), ids.next()) {
        (Some(id), None) => is_plain(id.as_bytes()),
        _ => false,
    };
    if !plain {
        request.headers_mut().remove(&HEADER);
    }
    request
}

fn is_plain(id: &[u8]) -> bool {
    (1..=LONGEST).contains(&id.len())
        && id
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The span a request runs in: its id, method and path. The query is left
/// out, so nothing a client puts there reaches the log.
pub(crate) fn span<B>(request: &axum::http::Request<B>) -> Span {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_id_is_short_letters_digits_dots_underscores_and_dashes() {
        let longest = "a".repeat(LONGEST);
        for plain in ["a", "proxy.7f3a_B-9", &longest] {
            assert!(is_plain(plain.as_bytes()), "{plain:?}");
        }

        let too_long = "a".repeat(LONGEST + 1);
        for odd in ["", &too_long, "two words", "a/b", "a\tb", "ü"] {
            assert!(!is_plain(odd.as_bytes()), "{odd:?}");
        }
    }
}
