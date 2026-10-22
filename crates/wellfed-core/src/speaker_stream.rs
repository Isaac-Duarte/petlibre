//! Speaker variant: the `PLSPK1` handshake that opens an audio stream.
//!
//! The feeder listens on [`PORT`]. On each connection it sends a hello: the
//! magic and a fresh challenge. The client answers with a header (magic,
//! sample rate, zero flags) and an HMAC-SHA256 under the FW08 request key
//! over the challenge and those fields, so only the key holder can play and a
//! recorded session cannot be replayed. The feeder replies with one status
//! byte; after `Ok` the rest of the connection is raw 16-bit little-endian
//! mono PCM at that rate, ending when the client closes.

use crate::request_auth::{KEY_LEN, hmac_sha256};

pub const PORT: u16 = 7700;
pub const MAGIC: [u8; 8] = *b"PLSPK1\0\0";
pub const CHALLENGE_LEN: usize = 16;
pub const HELLO_LEN: usize = MAGIC.len() + CHALLENGE_LEN;
/// Magic, rate (u32 LE), flags (u32 LE, zero), MAC.
pub const HEADER_LEN: usize = MAGIC.len() + 8 + 32;
/// The codec's rates (the I2S PLL divides to these).
pub const RATES: [u32; 4] = [8_000, 16_000, 32_000, 48_000];
/// Domain separation from signed transport requests under the same key.
const CONTEXT: &[u8] = b"PLSPK1 stream";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    Unauthenticated = 1,
    BadHeader = 2,
    Busy = 3,
    Muted = 4,
}

impl Status {
    pub fn from_byte(byte: u8) -> Option<Self> {
        [
            Self::Ok,
            Self::Unauthenticated,
            Self::BadHeader,
            Self::Busy,
            Self::Muted,
        ]
        .into_iter()
        .find(|status| *status as u8 == byte)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unauthenticated => "unauthenticated",
            Self::BadHeader => "bad_header",
            Self::Busy => "busy",
            Self::Muted => "muted",
        }
    }
}

pub fn hello(challenge: &[u8; CHALLENGE_LEN]) -> [u8; HELLO_LEN] {
    let mut out = [0; HELLO_LEN];
    out[..8].copy_from_slice(&MAGIC);
    out[8..].copy_from_slice(challenge);
    out
}

/// The challenge in a feeder's hello.
pub fn challenge(hello: &[u8; HELLO_LEN]) -> Option<[u8; CHALLENGE_LEN]> {
    (hello[..8] == MAGIC).then(|| hello[8..].try_into().expect("16 bytes"))
}

fn mac(key: &[u8; KEY_LEN], challenge: &[u8; CHALLENGE_LEN], fields: &[u8]) -> [u8; 32] {
    hmac_sha256(key, &[CONTEXT, challenge, fields])
}

/// The client's signed header for `rate`.
pub fn header(key: &[u8; KEY_LEN], challenge: &[u8; CHALLENGE_LEN], rate: u32) -> [u8; HEADER_LEN] {
    let mut out = [0; HEADER_LEN];
    out[..8].copy_from_slice(&MAGIC);
    out[8..12].copy_from_slice(&rate.to_le_bytes());
    let tag = mac(key, challenge, &out[..16]);
    out[16..].copy_from_slice(&tag);
    out
}

/// The rate a header asks for, once its MAC and fields check out.
pub fn check(
    key: &[u8; KEY_LEN],
    challenge: &[u8; CHALLENGE_LEN],
    header: &[u8; HEADER_LEN],
) -> Result<u32, Status> {
    let expected = mac(key, challenge, &header[..16]);
    let difference = expected
        .iter()
        .zip(&header[16..])
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if difference != 0 {
        return Err(Status::Unauthenticated);
    }

    let rate = u32::from_le_bytes(header[8..12].try_into().expect("4 bytes"));
    let flags = u32::from_le_bytes(header[12..16].try_into().expect("4 bytes"));
    if header[..8] != MAGIC || flags != 0 || !RATES.contains(&rate) {
        return Err(Status::BadHeader);
    }
    Ok(rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; KEY_LEN] = [7; KEY_LEN];
    const CHALLENGE: [u8; CHALLENGE_LEN] = [0x42; CHALLENGE_LEN];

    #[test]
    fn a_signed_header_opens_at_its_rate() {
        let hello = hello(&CHALLENGE);
        assert_eq!(challenge(&hello), Some(CHALLENGE));
        assert_eq!(
            check(&KEY, &CHALLENGE, &header(&KEY, &CHALLENGE, 32_000)),
            Ok(32_000)
        );
    }

    #[test]
    fn wrong_key_challenge_or_tamper_is_unauthenticated() {
        let good = header(&KEY, &CHALLENGE, 16_000);
        assert_eq!(
            check(&[8; KEY_LEN], &CHALLENGE, &good),
            Err(Status::Unauthenticated)
        );
        assert_eq!(
            check(&KEY, &[0x43; CHALLENGE_LEN], &good),
            Err(Status::Unauthenticated),
            "a recorded header is useless against a new challenge"
        );
        let mut tampered = good;
        tampered[8] ^= 1;
        assert_eq!(
            check(&KEY, &CHALLENGE, &tampered),
            Err(Status::Unauthenticated)
        );
    }

    #[test]
    fn unsupported_rates_are_refused_even_when_signed() {
        for rate in [0, 44_100, 96_000] {
            assert_eq!(
                check(&KEY, &CHALLENGE, &header(&KEY, &CHALLENGE, rate)),
                Err(Status::BadHeader)
            );
        }
        let mut hello = hello(&CHALLENGE);
        hello[0] = b'X';
        assert_eq!(challenge(&hello), None);
    }

    #[test]
    fn status_bytes_round_trip() {
        for byte in 0..=4 {
            assert_eq!(Status::from_byte(byte).map(|s| s as u8), Some(byte));
        }
        assert_eq!(Status::from_byte(5), None);
    }
}
