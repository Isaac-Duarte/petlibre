//! Conversions between Rust values and the SQLite columns they are stored
//! in. SQLite integers are `i64`, so an unsigned value that does not fit is
//! refused on the way in ([`Error::TooLarge`]) and a stored value out of
//! range is reported on the way out ([`Error::Corrupt`]).

use jiff::Timestamp;

use crate::{Error, Result};

pub(crate) fn millis(at: Timestamp) -> i64 {
    at.as_millisecond()
}

pub(crate) fn timestamp(ms: i64) -> Result<Timestamp> {
    Timestamp::from_millisecond(ms).map_err(|_| Error::Corrupt("time"))
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
