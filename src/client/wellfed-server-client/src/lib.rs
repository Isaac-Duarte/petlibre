//! A typed client of the owner's server HTTP API ([API design]), over
//! `wellfed-api`'s bodies: the CLI's `wellfed server` commands and the
//! server's own tests go through it, so neither can disagree with the
//! routes.
//!
//! A [`Client`] holds the server's base URL and how it authenticates
//! ([`Auth`]): the session cookie a login sets, or a bearer token. Each
//! typed method sends one request and accepts only its route's success
//! status; anything else comes back as an [`Error`], a refusal as the
//! server's typed [`Problem`]. [`Client::stream`] follows the server's
//! event stream.
//!
//! Writes carry the caller's `Idempotency-Key`, and nothing here retries:
//! a write that ends in [`Error::Http`] or `outcome_unknown` may have run,
//! so the caller retries it under the same key or not at all.
//!
//! [`Client::request`] is the escape hatch for a request the typed methods
//! cannot make (a malformed one, in a test), with the same base, origin
//! and credentials.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod auth;
mod commands;
mod error;
mod feeders;
mod feeds;
mod foods;
mod health;
mod household;
mod pets;
mod releases;
mod stream;
mod tokens;
mod users;

use std::fmt;
use std::time::Duration;

use wellfed_api::{PREFIX, Problem, SESSION_COOKIE};
use reqwest::header::{self, HeaderValue};
use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;

pub use error::Error;
pub use feeders::Lock;
/// The API's bodies, for a caller that may not depend on `wellfed-api`
/// itself: the CLI reaches the server through this crate alone.
pub use wellfed_api as api;
pub use releases::Uploaded;
pub use reqwest::{self, Method, StatusCode, Url};
pub use stream::{Update, Updates};

/// How long a connection to the server may take to open. A server on the
/// LAN answers in milliseconds; a request's own time is the caller's to
/// bound, since an event stream stays open for good.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How a client proves who it is.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// The value of the session cookie a login set.
    Session(String),
    /// A bearer token.
    Token(String),
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Either one is a secret.
        match self {
            Self::Session(_) => f.write_str("Session(..)"),
            Self::Token(_) => f.write_str("Token(..)"),
        }
    }
}

/// A client of one server. Cloning it is cheap and shares the connection
/// pool; each clone keeps its own credentials.
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    /// The base URL, without a trailing slash.
    base: String,
    /// The base URL's origin, sent with every request.
    origin: HeaderValue,
    auth: Option<Auth>,
}

impl Client {
    /// A client of the server at `base`, an `http` or `https` URL such as
    /// `http://192.0.2.7:8080` (a path is kept as a prefix, for a server
    /// behind a reverse proxy), with no credentials yet.
    pub fn new(base: &Url) -> Result<Self, Error> {
        let http_url = matches!(base.scheme(), "http" | "https") && base.host().is_some();
        if !http_url || base.query().is_some() || base.fragment().is_some() {
            return Err(Error::Base(base.clone()));
        }
        let origin = HeaderValue::from_str(&base.origin().ascii_serialization())
            .map_err(|_| Error::Base(base.clone()))?;

        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(Error::Build)?;

        Ok(Self {
            http,
            base: base.as_str().trim_end_matches('/').to_owned(),
            origin,
            auth: None,
        })
    }

    /// This client with `auth` in place of its credentials.
    #[must_use]
    pub fn with_auth(mut self, auth: Option<Auth>) -> Self {
        self.auth = auth;
        self
    }

    pub fn auth(&self) -> Option<&Auth> {
        self.auth.as_ref()
    }

    /// The URL of `path`, from the server's root (`/health`, `/api/v1/…`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The underlying HTTP client, for a request that must not carry this
    /// client's origin or credentials.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// A request for `path` from the server's root, carrying the origin
    /// and the credentials every typed method sends.
    ///
    /// The server refuses a cookie-authenticated write whose `Origin` is
    /// not its own, to stop other sites' pages; this client is not a
    /// page, so it names the server's origin as the server's own pages do.
    /// A token-authenticated write needs no `Origin`, and carries it all
    /// the same.
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self
            .http
            .request(method, self.url(path))
            .header(header::ORIGIN, self.origin.clone());

        match &self.auth {
            Some(Auth::Session(value)) => {
                request.header(header::COOKIE, format!("{SESSION_COOKIE}={value}"))
            }
            Some(Auth::Token(token)) => request.bearer_auth(token),
            None => request,
        }
    }

    /// A request for `path` under the API's prefix.
    fn api(&self, method: Method, path: &str) -> RequestBuilder {
        self.request(method, &format!("{PREFIX}{path}"))
    }
}

/// `response` if its status is `expected`. Any other status is the
/// server's [`Error::Problem`] when it sent one, else
/// [`Error::Unexpected`].
pub async fn expect_status(response: Response, expected: StatusCode) -> Result<Response, Error> {
    let status = response.status();
    if status == expected {
        return Ok(response);
    }

    if is_problem(&response) {
        let problem: Problem = serde_json::from_slice(&response.bytes().await?)?;
        return Err(Error::Problem {
            status,
            problem: Box::new(problem),
        });
    }
    Err(Error::Unexpected { status, expected })
}

/// The JSON body of `response`, if its status is `expected`.
pub async fn decode<T: DeserializeOwned>(
    response: Response,
    expected: StatusCode,
) -> Result<T, Error> {
    let response = expect_status(response, expected).await?;
    Ok(serde_json::from_slice(&response.bytes().await?)?)
}

/// Whether `response` carries an RFC 9457 problem, whatever parameters
/// (`; charset=utf-8`) its media type has.
fn is_problem(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|kind| kind.to_str().ok())
        .is_some_and(is_problem_type)
}

fn is_problem_type(content_type: &str) -> bool {
    let media = content_type.split(';').next().unwrap_or_default();
    media
        .trim()
        .eq_ignore_ascii_case("application/problem+json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(base: &str) -> Result<Client, Error> {
        Client::new(&Url::parse(base).expect("a URL"))
    }

    #[test]
    fn the_base_is_an_http_url_and_its_origin_goes_with_every_request() {
        let client = open("http://192.0.2.7:8080/").expect("an http base");
        assert_eq!(client.url("/health"), "http://192.0.2.7:8080/health");

        let request = client
            .with_auth(Some(Auth::Session("abc".into())))
            .api(Method::POST, "/setup")
            .build()
            .expect("a request");
        assert_eq!(request.url().as_str(), "http://192.0.2.7:8080/api/v1/setup");
        assert_eq!(request.headers()[header::ORIGIN], "http://192.0.2.7:8080");
        assert_eq!(request.headers()[header::COOKIE], "wellfed_session=abc");

        for base in [
            "ftp://192.0.2.7",
            "unix:/run/server.sock",
            "http://192.0.2.7/?a=1",
            "http://192.0.2.7/#top",
        ] {
            assert!(matches!(open(base), Err(Error::Base(_))), "{base}");
        }
    }

    #[test]
    fn a_problem_is_its_media_type_with_any_parameters() {
        for kind in [
            "application/problem+json",
            "application/problem+json; charset=utf-8",
            "Application/Problem+JSON ;charset=UTF-8",
        ] {
            assert!(is_problem_type(kind), "{kind}");
        }
        for kind in ["application/json", "application/problem+jsonx", ""] {
            assert!(!is_problem_type(kind), "{kind}");
        }
    }

    #[test]
    fn a_token_is_a_bearer_and_neither_secret_is_printed() {
        let client = open("https://feeder.example")
            .expect("an https base")
            .with_auth(Some(Auth::Token("secret".into())));
        let request = client
            .api(Method::GET, "/feeders")
            .build()
            .expect("a request");
        assert_eq!(request.headers()[header::AUTHORIZATION], "Bearer secret");
        assert!(request.headers().get(header::COOKIE).is_none());
        assert!(!format!("{client:?}").contains("secret"));
    }
}
