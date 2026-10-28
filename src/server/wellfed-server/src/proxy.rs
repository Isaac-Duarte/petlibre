//! A request's client behind a reverse proxy: its address and the scheme
//! it used, taken from forwarded headers only when the connection comes
//! from a proxy the owner trusts (`trusted_proxies`).
//!
//! From anyone else those headers are the client's own words, so its
//! address is the connection's and its scheme plain HTTP, which is all
//! this server serves. Behind a trusted proxy the address is read from
//! `X-Forwarded-For`, or `Forwarded` when that is absent, right to left:
//! each proxy appends the address it heard from, so the first one that is
//! not itself a trusted proxy is the client, and anything left of it is
//! whatever the client chose to send. The header is chosen by presence:
//! when `X-Forwarded-For` is sent at all, `Forwarded` is ignored, so the
//! proxy must set (or append to) `X-Forwarded-For`, as Caddy and nginx's
//! `$proxy_add_x_forwarded_for` do; a client's own `Forwarded` then counts
//! for nothing. The scheme is `X-Forwarded-Proto`'s last value, or the
//! last `Forwarded` element's `proto`; `https` makes the session cookie
//! `Secure`. That is the scheme the last proxy reports: with a chain of
//! proxies, the one in front must set it from the client's connection and
//! each behind it pass it on, since no hop's value is checked against the
//! one before.

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName};
use serde::Deserialize;
use wellfed_api::ErrorCode;

use crate::AppState;
use crate::error::ApiError;

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
const FORWARDED: HeaderName = HeaderName::from_static("forwarded");

/// A proxy's address, or a range of them in CIDR form (`10.0.0.0/8`,
/// `fd00::/8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ProxyRange {
    network: IpAddr,
    prefix: u8,
}

/// Not an address or a CIDR range.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{text}` is not an IP address or a CIDR range such as 10.0.0.0/8")]
pub struct BadRange {
    text: String,
}

impl ProxyRange {
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                masked(u128::from(network.to_bits()), 32, self.prefix)
                    == masked(u128::from(address.to_bits()), 32, self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                masked(network.to_bits(), 128, self.prefix)
                    == masked(address.to_bits(), 128, self.prefix)
            }
            _ => false,
        }
    }
}

/// The top `prefix` bits of a `width`-bit address.
fn masked(bits: u128, width: u8, prefix: u8) -> u128 {
    let host = u32::from(width - prefix);
    let shifted = bits.checked_shr(host).unwrap_or(0);
    shifted.checked_shl(host).unwrap_or(0)
}

impl FromStr for ProxyRange {
    type Err = BadRange;

    fn from_str(text: &str) -> Result<Self, BadRange> {
        let bad = || BadRange {
            text: text.to_owned(),
        };
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let network: IpAddr = address.parse().map_err(|_| bad())?;
        let width = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().map_err(|_| bad())?,
            None => width,
        };
        if prefix > width {
            return Err(bad());
        }

        // An IPv4-mapped range (`::ffff:10.0.0.0/104`) is its IPv4 one, as
        // the addresses it is matched against are.
        match network {
            IpAddr::V6(v6) if prefix >= 96 => match v6.to_ipv4_mapped() {
                Some(v4) => Ok(Self {
                    network: IpAddr::V4(v4),
                    prefix: prefix - 96,
                }),
                None => Ok(Self { network, prefix }),
            },
            _ => Ok(Self { network, prefix }),
        }
    }
}

impl TryFrom<String> for ProxyRange {
    type Error = BadRange;

    fn try_from(text: String) -> Result<Self, BadRange> {
        text.parse()
    }
}

impl std::fmt::Display for ProxyRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

/// The proxies whose forwarded headers are believed.
#[derive(Clone, Debug, Default)]
pub struct TrustedProxies(Vec<ProxyRange>);

impl TrustedProxies {
    pub fn new(ranges: impl IntoIterator<Item = ProxyRange>) -> Self {
        Self(ranges.into_iter().collect())
    }

    fn trusts(&self, address: IpAddr) -> bool {
        self.0.iter().any(|range| range.contains(address))
    }

    /// The client's address and scheme, for a request that came over a
    /// connection from `peer`.
    pub(crate) fn client(&self, peer: IpAddr, headers: &HeaderMap) -> (IpAddr, Scheme) {
        if !self.trusts(peer) {
            return (peer, Scheme::Http);
        }

        let mut client = peer;
        for hop in forwarded_for(headers).iter().rev() {
            // An entry that names no address (`unknown`, an obfuscated
            // name) leaves the last proxy that could be named.
            let Some(hop) = *hop else { break };
            client = hop;
            if !self.trusts(hop) {
                break;
            }
        }
        (client, scheme(headers))
    }
}

/// The client a request came from, through a trusted proxy or not.
pub(crate) struct Remote {
    pub(crate) address: IpAddr,
    pub(crate) scheme: Scheme,
}

impl FromRequestParts<AppState> for Remote {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        // The server is always served with its connections' addresses.
        let Some(ConnectInfo(peer)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() else {
            return Err(ApiError::Code(
                ErrorCode::Internal,
                Some("the connection's address is unknown".into()),
            ));
        };
        let (address, scheme) = state.proxies.client(peer.ip(), &parts.headers);
        Ok(Self { address, scheme })
    }
}

/// How the client reached the proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scheme {
    Http,
    Https,
}

/// Every hop's address, left to right: `X-Forwarded-For`'s when it is
/// sent, else `Forwarded`'s `for`. `None` for a hop that names none.
fn forwarded_for(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    if headers.contains_key(X_FORWARDED_FOR) {
        let hops = values(headers, &X_FORWARDED_FOR);
        return hops
            .iter()
            .map(|hop| hop.as_deref().and_then(node))
            .collect();
    }
    values(headers, &FORWARDED)
        .iter()
        .map(|element| {
            element
                .as_deref()
                .and_then(|element| parameter(element, "for"))
                .and_then(node)
        })
        .collect()
}

/// The scheme the last hop reports: `X-Forwarded-Proto`'s last value when
/// that header is sent at all, else the last `Forwarded` element's.
fn scheme(headers: &HeaderMap) -> Scheme {
    let proto = if headers.contains_key(X_FORWARDED_PROTO) {
        values(headers, &X_FORWARDED_PROTO).pop().flatten()
    } else {
        let elements = values(headers, &FORWARDED);
        elements
            .last()
            .and_then(Option::as_deref)
            .and_then(|element| parameter(element, "proto"))
            .map(str::to_owned)
    };
    match proto {
        Some(proto) if proto.eq_ignore_ascii_case("https") => Scheme::Https,
        _ => Scheme::Http,
    }
}

/// A header's comma-separated values across all its lines, in order, split
/// on the raw bytes: a value that is not text is `None` and keeps its
/// place, so a client's garbage left of a proxy's entry never hides that
/// entry, nor makes the header read as absent.
fn values(headers: &HeaderMap, name: &HeaderName) -> Vec<Option<String>> {
    headers
        .get_all(name)
        .iter()
        .flat_map(|line| line.as_bytes().split(|&byte| byte == b','))
        .filter(|value| !value.trim_ascii().is_empty())
        .map(|value| {
            std::str::from_utf8(value.trim_ascii())
                .ok()
                .map(str::to_owned)
        })
        .collect()
}

/// A `Forwarded` element's parameter (`for=`, `proto=`), unquoted.
fn parameter<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    element.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().trim_matches('"'))
    })
}

/// A node's address: `192.0.2.7`, `192.0.2.7:4711`, `2001:db8::1` or
/// `[2001:db8::1]:4711`, quoted or not.
fn node(text: &str) -> Option<IpAddr> {
    let text = text.trim().trim_matches('"');
    if let Some(bracketed) = text.strip_prefix('[') {
        return bracketed.split_once(']')?.0.parse().ok();
    }
    text.parse().ok().or_else(|| {
        let (address, _port) = text.rsplit_once(':')?;
        address.parse::<std::net::Ipv4Addr>().ok().map(IpAddr::V4)
    })
}

#[cfg(test)]
mod tests;
