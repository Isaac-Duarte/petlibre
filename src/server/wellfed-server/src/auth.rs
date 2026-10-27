//! Who is asking, and whether a write came from this server's own pages.
//!
//! A browser holds an `HttpOnly`, `SameSite=Strict` session cookie. Every
//! write must also carry an `Origin` naming this server, and
//! `Sec-Fetch-Site: same-origin` when the browser sends it: owners mostly
//! use plain HTTP on the LAN, where browsers omit Fetch Metadata, so
//! `Origin` is the check that always applies.
//!
//! A phone or the CLI sends `Authorization: Bearer <token>` instead, and
//! its writes need no `Origin`: a browser attaches a cookie to another
//! site's request on its own, but never an `Authorization` header, which a
//! page could set only through a CORS preflight this server never grants.
//! A request that carries `Authorization` is authenticated by it alone,
//! never by a cookie beside it, so skipping the check for it cannot let a
//! cookie through unchecked.
//!
//! Comparing `Origin` with `Host` does not stop DNS rebinding, where an
//! attacker's own name points at this server and so appears in both. So
//! every request must also name a host the server answers to: an IP
//! address, `localhost`, or a name it was started with ([`Hosts`]). A
//! rebinding page can only use its own name, which is none of these.

use std::marker::PhantomData;
use std::net::IpAddr;
use std::time::Duration;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::CookieJar;
use wellfed_api::{BEARER, ErrorCode, SESSION_COOKIE as COOKIE};
use wellfed_service::{BearerToken, Caller, Permission, SessionToken, can};

use crate::AppState;
use crate::error::ApiError;
use crate::proxy::Scheme;

/// What a request authenticates with.
pub(crate) enum Credential {
    Session(SessionToken),
    Token(BearerToken),
}

impl Credential {
    /// Who it stands for while it holds.
    pub(crate) async fn caller(&self, state: &AppState) -> Result<Option<Caller>, ApiError> {
        Ok(match self {
            Self::Session(token) => state.service.authenticate(token).await?.map(Caller::from),
            Self::Token(token) => state.service.authenticate_token(token).await?,
        })
    }
}

/// The request's credential: its `Authorization` header when it has one,
/// otherwise its session cookie. A malformed `Authorization` is no
/// credential at all, never a fall back to the cookie.
pub(crate) fn credential(headers: &HeaderMap) -> Option<Credential> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        return bearer(value).map(Credential::Token);
    }
    let jar = CookieJar::from_headers(headers);
    jar.get(COOKIE)
        .map(|cookie| Credential::Session(SessionToken::new(cookie.value().to_owned())))
}

/// The session a request came with, when that is how it authenticates.
pub(crate) fn session(headers: &HeaderMap) -> Option<SessionToken> {
    match credential(headers) {
        Some(Credential::Session(token)) => Some(token),
        Some(Credential::Token(_)) | None => None,
    }
}

fn bearer(value: &HeaderValue) -> Option<BearerToken> {
    let (scheme, secret) = value.to_str().ok()?.split_once(' ')?;
    let secret = secret.trim();
    (scheme.eq_ignore_ascii_case(BEARER) && !secret.is_empty())
        .then(|| BearerToken::new(secret.to_owned()))
}

/// The caller: a logged-in user, or a token's.
pub(crate) struct Auth(pub(crate) Caller);

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let unauthenticated = || ApiError::Code(ErrorCode::Unauthenticated, None);
        let credential = credential(&parts.headers).ok_or_else(unauthenticated)?;
        let caller = credential.caller(state).await?;
        caller.map(Self).ok_or_else(unauthenticated)
    }
}

/// The caller, whose role [`can`] do `P`'s permission: each route names
/// the one it needs (`Allowed<need::Control>`), and the service's table
/// decides. A token has no role of its own: its user's, as it is now.
pub(crate) struct Allowed<P>(pub(crate) Caller, pub(crate) PhantomData<P>);

/// A permission a route names as a type.
pub(crate) trait Needs {
    const PERMISSION: Permission;
}

impl<P: Needs> FromRequestParts<AppState> for Allowed<P> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let Auth(caller) = Auth::from_request_parts(parts, state).await?;
        let role = caller.user.role;
        if !can(role, P::PERMISSION) {
            return Err(ApiError::Code(
                ErrorCode::Forbidden,
                Some(format!("a {} may not do that", role.code())),
            ));
        }
        Ok(Self(caller, PhantomData))
    }
}

/// The routes' permissions, one type each.
pub(crate) mod need {
    use wellfed_service::Permission;

    use super::Needs;

    macro_rules! needs {
        ($($name:ident),* $(,)?) => {$(
            pub(crate) enum $name {}

            impl Needs for $name {
                const PERMISSION: Permission = Permission::$name;
            }
        )*};
    }

    needs!(
        Read,
        Feed,
        Control,
        OwnHistory,
        OwnAccount,
        Users,
        Household,
        OwnerData,
        FeederAdmin,
        Adopt,
        ServerData,
    );
}

/// How long a browser keeps the session cookie: the longest browsers
/// allow. The server ends a session a fixed time after its last use, so the
/// cookie must outlive any session rather than end a fixed time after the
/// login.
const COOKIE_LIFETIME: Duration = Duration::from_secs(400 * 24 * 3600);

/// The `Set-Cookie` value that carries a new session, `Secure` when the
/// client reached a trusted proxy over HTTPS. The token is hex, so it
/// needs no quoting.
pub(crate) fn session_cookie(token: &SessionToken, scheme: Scheme) -> String {
    let max_age = COOKIE_LIFETIME.as_secs();
    format!(
        "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}{}",
        token.as_str(),
        secure(scheme),
    )
}

/// The `Set-Cookie` value that ends one.
pub(crate) fn cleared_cookie(scheme: Scheme) -> String {
    format!(
        "{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{}",
        secure(scheme)
    )
}

const fn secure(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Https => "; Secure",
        Scheme::Http => "",
    }
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
/// `SameSite=Strict` keeps the cookie off cross-site requests anyway. A
/// write with a bearer token passes: no other site's page can send one.
pub(crate) async fn same_origin(request: Request, next: Next) -> Result<Response, ApiError> {
    let headers = request.headers();
    let token = matches!(credential(headers), Some(Credential::Token(_)));
    if !request.method().is_safe() && !token && !from_this_server(headers) {
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
    fn an_authorization_header_is_the_only_credential_when_sent() {
        let cookie = ("cookie", "wellfed_session=00ff");
        let kind = |pairs: &[(&'static str, &'static str)]| match credential(&headers(pairs)) {
            Some(Credential::Session(token)) => Some(("session", token.as_str().to_owned())),
            Some(Credential::Token(token)) => Some(("token", token.as_str().to_owned())),
            None => None,
        };

        assert_eq!(kind(&[cookie]), Some(("session", "00ff".into())));
        assert_eq!(
            kind(&[cookie, ("authorization", "Bearer plt_ab")]),
            Some(("token", "plt_ab".into()))
        );
        assert_eq!(
            kind(&[("authorization", "bearer  plt_ab ")]),
            Some(("token", "plt_ab".into()))
        );
        for odd in ["Bearer", "Bearer ", "Basic YWRh", "plt_ab"] {
            assert_eq!(kind(&[cookie, ("authorization", odd)]), None, "{odd}");
        }
        assert_eq!(kind(&[]), None);
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
