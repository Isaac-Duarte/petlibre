//! Who is asking, and whether a write came from this server's own pages.
//!
//! A browser holds an `HttpOnly`, `SameSite=Strict` session cookie. Every
//! write must also carry an `Origin` naming this server, and
//! `Sec-Fetch-Site: same-origin` when the browser sends it: owners mostly
//! use plain HTTP on the LAN, where browsers omit Fetch Metadata, so
//! `Origin` is the check that always applies.
//!
//! Comparing `Origin` with `Host` does not stop DNS rebinding, where an
//! attacker's own name points at this server and so appears in both. So
//! every request must also name a host the server answers to: an IP
//! address, `localhost`, or a name it was started with ([`Hosts`]). A
//! rebinding page can only use its own name, which is none of these.

use std::net::IpAddr;
use std::time::Duration;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::CookieJar;
use wellfed_api::{ErrorCode, Role, User};
use wellfed_service::SessionToken;

use crate::AppState;
use crate::error::ApiError;

pub(crate) const COOKIE: &str = "wellfed_session";

/// The logged-in user.
pub(crate) struct Auth(pub(crate) User);

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let unauthenticated = || ApiError::Code(ErrorCode::Unauthenticated, None);
        let token = session_token(&parts.headers).ok_or_else(unauthenticated)?;
        let user = state.service.authenticate(&token).await?;
        user.map(Self).ok_or_else(unauthenticated)
    }
}

/// The logged-in user, who must be an owner.
pub(crate) struct Owner;

impl FromRequestParts<AppState> for Owner {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let Auth(user) = Auth::from_request_parts(parts, state).await?;
        if user.role != Role::Owner {
            return Err(ApiError::Code(
                ErrorCode::Forbidden,
                Some("only an owner may do that".into()),
            ));
        }
        Ok(Self)
    }
}

pub(crate) fn session_token(headers: &HeaderMap) -> Option<SessionToken> {
    let jar = CookieJar::from_headers(headers);
    jar.get(COOKIE)
        .map(|cookie| SessionToken::new(cookie.value().to_owned()))
}

/// The `Set-Cookie` value that carries a new session. The token is hex, so
/// it needs no quoting.
pub(crate) fn session_cookie(token: &SessionToken, lifetime: Duration) -> String {
    let max_age = lifetime.as_secs();
    format!(
        "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}",
        token.as_str()
    )
}

/// The `Set-Cookie` value that ends one.
pub(crate) fn cleared_cookie() -> String {
    format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

/// The names this server answers to besides IP addresses and `localhost`,
/// such as `feeder.lan`.
#[derive(Clone, Debug, Default)]
pub struct Hosts(Vec<String>);

impl Hosts {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self(
            names
                .into_iter()
                .map(|name| name.to_ascii_lowercase())
                .collect(),
        )
    }

    /// Whether a `Host` header (with or without its port) names this server.
    fn allows(&self, host: &str) -> bool {
        let name = match host.strip_prefix('[') {
            Some(v6) => v6.split_once(']').map_or(v6, |(address, _)| address),
            None => host.rsplit_once(':').map_or(host, |(name, _)| name),
        };
        name.parse::<IpAddr>().is_ok()
            || name.eq_ignore_ascii_case("localhost")
            || self.0.iter().any(|known| known.eq_ignore_ascii_case(name))
    }
}

/// Refuse a request for a host this server does not answer to.
pub(crate) async fn known_host(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    if !host.is_some_and(|host| state.hosts.allows(host)) {
        return Err(ApiError::Code(
            ErrorCode::UnknownHost,
            Some("use the server's address, or start it with --host for this name".into()),
        ));
    }
    Ok(next.run(request).await)
}

/// Refuse a write from another site. Reads pass: they change nothing, and
/// `SameSite=Strict` keeps the cookie off cross-site requests anyway.
pub(crate) async fn same_origin(request: Request, next: Next) -> Result<Response, ApiError> {
    if !request.method().is_safe() && !from_this_server(request.headers()) {
        return Err(ApiError::Code(
            ErrorCode::CrossOrigin,
            Some("a write must come from this server's own pages".into()),
        ));
    }
    Ok(next.run(request).await)
}

fn from_this_server(headers: &HeaderMap) -> bool {
    let site = headers
        .get("sec-fetch-site")
        .map(|site| site.as_bytes() == b"same-origin");
    if site == Some(false) {
        return false;
    }
    let (Some(origin), Some(host)) = (headers.get(header::ORIGIN), headers.get(header::HOST))
    else {
        return false;
    };
    let (Ok(origin), Ok(host)) = (origin.to_str(), host.to_str()) else {
        return false;
    };
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    header::HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    #[test]
    fn addresses_localhost_and_named_hosts_are_known() {
        let hosts = Hosts::new(["Feeder.lan".to_owned()]);
        for host in [
            "192.0.2.7",
            "192.0.2.7:8080",
            "[::1]:8080",
            "[fe80::1]",
            "localhost:8080",
            "feeder.lan",
            "FEEDER.LAN:8080",
        ] {
            assert!(hosts.allows(host), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:8080",
            "feeder.lan.evil.example",
            "",
        ] {
            assert!(!hosts.allows(host), "{host}");
        }
    }

    #[test]
    fn only_this_servers_origin_may_write() {
        let host = ("host", "feeder.lan:8080");
        assert!(from_this_server(&headers(&[
            host,
            ("origin", "http://feeder.lan:8080")
        ])));
        assert!(from_this_server(&headers(&[
            host,
            ("origin", "https://FEEDER.lan:8080")
        ])));
        assert!(from_this_server(&headers(&[
            host,
            ("origin", "http://feeder.lan:8080"),
            ("sec-fetch-site", "same-origin"),
        ])));

        assert!(!from_this_server(&headers(&[host])), "no origin");
        assert!(!from_this_server(&headers(&[
            host,
            ("origin", "http://evil.example")
        ])));
        assert!(!from_this_server(&headers(&[
            host,
            ("origin", "http://feeder.lan")
        ])));
        assert!(!from_this_server(&headers(&[
            host,
            ("origin", "http://feeder.lan:8080"),
            ("sec-fetch-site", "cross-site"),
        ])));
        assert!(!from_this_server(&headers(&[host, ("origin", "null")])));
    }
}
