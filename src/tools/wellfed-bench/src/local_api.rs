//! `wellfed-local serve`'s loopback HTTP API as the bench speaks it: the
//! integration suite drives it, and the page reads its status.
//!
//! One plain HTTP/1.1 exchange per call over a fresh connection; the server
//! answers JSON and closes, so no client library is needed.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

/// The server config the suite serves with and the page reads, root-relative.
pub(crate) const SERVER_CONFIG: &str = "private/server.json";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The parts of `wellfed-local`'s config the bench uses.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ServerConfig {
    pub api_token: String,
    pub http_listen: SocketAddr,
    pub mqtt_listen: SocketAddr,
}

/// Whether an API call carries the server's bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Auth {
    Token,
    /// `/health`, which answers anyone on loopback.
    Anonymous,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalApi {
    pub address: SocketAddr,
    token: String,
    timeout: Duration,
}

impl LocalApi {
    pub(crate) fn new(config: &ServerConfig, timeout: Duration) -> Self {
        Self {
            address: config.http_listen,
            token: config.api_token.clone(),
            timeout,
        }
    }

    /// One exchange; the status code and JSON body.
    fn http(&self, method: &str, path: &str, auth: Auth, body: &str) -> Result<(u16, Value)> {
        let mut stream = TcpStream::connect_timeout(&self.address, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            self.address,
            body.len()
        );
        if auth == Auth::Token {
            request += &format!("Authorization: Bearer {}\r\n", self.token);
        }
        request += "\r\n";
        stream.write_all(request.as_bytes())?;
        stream.write_all(body.as_bytes())?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        parse_response(&response)
    }

    pub(crate) fn get<T: DeserializeOwned>(&self, path: &str, auth: Auth) -> Result<T> {
        match self.http("GET", path, auth, "")? {
            (200, value) => Ok(serde_json::from_value(value)?),
            (code, value) => Err(Error::Bench(format!("GET {path}: {code} {value}"))),
        }
    }

    pub(crate) fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let body = serde_json::to_string(body)?;
        match self.http("POST", path, Auth::Token, &body)? {
            (200, value) => Ok(serde_json::from_value(value)?),
            (code, value) => Err(Error::Bench(format!("POST {path}: {code} {value}"))),
        }
    }
}

/// Split an HTTP/1.1 response into its status code and JSON body, decoding
/// a chunked body. The body stays a `Value` until the caller types it, so a
/// refusal's body can be shown whatever its shape.
pub(crate) fn parse_response(response: &[u8]) -> Result<(u16, Value)> {
    let text = String::from_utf8_lossy(response);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| Error::Bench("Malformed HTTP response".into()))?;
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| Error::Bench("HTTP response has no status code".into()))?;

    let chunked = head.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_string()
    };
    let value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body)?
    };
    Ok((code, value))
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || rest.len() < size {
            break;
        }
        out += &rest[..size];
        body = rest[size..].trim_start_matches("\r\n");
    }
    out
}
