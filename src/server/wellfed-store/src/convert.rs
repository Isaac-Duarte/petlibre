//! Conversions between Rust values and the SQLite columns they are stored
//! in. SQLite integers are `i64`, so an unsigned value that does not fit is
//! refused on the way in ([`Error::TooLarge`]) and a stored value out of
//! range is reported on the way out ([`Error::Corrupt`]).

use jiff::Timestamp;
use wellfed_api::{Hex, Source};

use crate::{Error, Result};

pub(crate) fn millis(at: Timestamp) -> i64 {
    at.as_millisecond()
}

pub(crate) fn timestamp(ms: i64) -> Result<Timestamp> {
    Timestamp::from_millisecond(ms).map_err(|_| Error::Corrupt("time"))
}

/// A feeder clock time, stored in whole seconds; `None` while that clock
/// was invalid.
pub(crate) fn seconds(at: Option<i64>) -> Result<Option<Timestamp>> {
    at.map(|s| Timestamp::from_second(s).map_err(|_| Error::Corrupt("event time")))
        .transpose()
}

/// Who caused an event, as its two columns: the kind, and the client's
/// fingerprint for a client only.
pub(crate) const fn source_columns(source: Source) -> (&'static str, Option<[u8; 4]>) {
    match source {
        Source::Panel => ("panel", None),
        Source::Client { client } => ("client", Some(client.0)),
        Source::Feeder => ("feeder", None),
    }
}

pub(crate) fn source(kind: &str, client: Option<Vec<u8>>) -> Result<Source> {
    match (kind, client) {
        ("panel", None) => Ok(Source::Panel),
        ("feeder", None) => Ok(Source::Feeder),
        ("client", Some(client)) => {
            let client = <[u8; 4]>::try_from(client.as_slice())
                .map_err(|_| Error::Corrupt("client fingerprint"))?;
            Ok(Source::Client {
                client: Hex(client),
            })
        }
        _ => Err(Error::Corrupt("event source")),
    }
}

pub(crate) fn to_i64(value: u64, what: &'static str) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::TooLarge(what))
}

pub(crate) fn from_i64(value: i64, what: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Corrupt(what))
}

/// A cached JSON value and the time it was read, when both are stored.
pub(crate) fn cached<T: serde::de::DeserializeOwned>(
    json: Option<String>,
    at: Option<i64>,
    what: &'static str,
) -> Result<Option<(T, Timestamp)>> {
    let (Some(json), Some(at)) = (json, at) else {
        return Ok(None);
    };
    let value = serde_json::from_str(&json).map_err(|_| Error::Corrupt(what))?;
    Ok(Some((value, timestamp(at)?)))
}
