//! The device protocol's messages ([API design]): what the feeder and its
//! clients say to each other once a Noise session carries their bytes.
//!
//! Messages are postcard-encoded serde types, so the feeder (no allocation),
//! the host client and the app's generated bindings read one definition.
//! Nothing here does IO or cryptography: the caller seals and opens the bytes.
//! FW45's BLE setup exchange is the first protocol here; FW47's device API
//! grows it.
//!
//! [API design]: ../../../../docs/api/README.md
#![no_std]

pub mod setup;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("the output buffer cannot hold the message")]
    BufferTooSmall,
    #[error("the bytes are not one well-formed message")]
    Malformed,
}

/// Encode `value` into `out`; the encoded length.
pub fn encode<T: Serialize>(value: &T, out: &mut [u8]) -> Result<usize, Error> {
    postcard::to_slice(value, out)
        .map(|used| used.len())
        .map_err(|error| match error {
            postcard::Error::SerializeBufferFull => Error::BufferTooSmall,
            _ => Error::Malformed,
        })
}

/// Decode exactly one message: trailing bytes are malformed, not ignored.
pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, Error> {
    match postcard::take_from_bytes(bytes) {
        Ok((value, [])) => Ok(value),
        _ => Err(Error::Malformed),
    }
}
