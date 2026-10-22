//! Authenticated server requests on the plaintext FW08 channel.
//!
//! The server signs each request with a per-device key built into the image:
//! the payload ends with `,"mac":"<64 hex>"}`, an HMAC-SHA256 over every byte
//! before `,"mac":"`. Replay is refused without persistent state: each MQTT
//! session the firmware draws a fresh 64-bit nonce and reports it in health,
//! and a request must name that nonce with a `seq` above the last accepted
//! one. A request from another session or an earlier `seq` fails.
//!
//! The nonce only has to differ between sessions under one key. It is the
//! HMAC of whatever entropy the board supplies (a ROM generator and timer
//! values at connect), so weak inputs still give unrelated nonces as long
//! as they differ.

use sha2::{Digest, Sha256};

pub const KEY_LEN: usize = 32;
/// `,"mac":"` + 64 hex + `"}`.
const SUFFIX: &[u8] = b",\"mac\":\"";
const MAC_HEX: usize = 64;

/// HMAC-SHA256 (RFC 2104) of the concatenated `parts`.
pub fn hmac_sha256(key: &[u8; KEY_LEN], parts: &[&[u8]]) -> [u8; 32] {
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for (i, byte) in key.iter().enumerate() {
        inner_pad[i] ^= byte;
        outer_pad[i] ^= byte;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    for part in parts {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// 64 lowercase hex characters as 32 bytes.
pub fn parse_key(hex: &[u8]) -> Option<[u8; KEY_LEN]> {
    if hex.len() != 2 * KEY_LEN {
        return None;
    }
    let mut key = [0u8; KEY_LEN];
    for (i, pair) in hex.as_chunks::<2>().0.iter().enumerate() {
        key[i] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Some(key)
}

/// A payload split at its MAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signed<'a> {
    /// Every byte the MAC covers; append `}` to parse it as a request.
    pub prefix: &'a [u8],
    pub mac: [u8; 32],
}

/// The signed form, or `None` for a payload that does not end in exactly
/// `,"mac":"<64 lowercase hex>"}`.
pub fn split(payload: &[u8]) -> Option<Signed<'_>> {
    let tail = SUFFIX.len() + MAC_HEX + 2;
    let at = payload.len().checked_sub(tail)?;
    let (prefix, rest) = payload.split_at(at);
    let hex = rest.strip_prefix(SUFFIX)?.strip_suffix(b"\"}")?;
    let mac = parse_key(hex)?;
    Some(Signed { prefix, mac })
}

/// Equal without an early exit, so timing does not reveal a partial match.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// One MQTT session's verifier.
#[derive(Clone, Copy, Debug)]
pub struct Session {
    key: [u8; KEY_LEN],
    nonce: u64,
    last_seq: u32,
}

impl Session {
    pub fn new(key: [u8; KEY_LEN], entropy: &[u8]) -> Self {
        let digest = hmac_sha256(&key, &[b"petlibre-request-nonce", entropy]);
        let mut first = [0u8; 8];
        first.copy_from_slice(&digest[..8]);
        Self {
            key,
            nonce: u64::from_be_bytes(first),
            last_seq: 0,
        }
    }

    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// The nonce as health and requests write it: 16 lowercase hex characters.
    pub fn nonce_hex(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, slot) in out.iter_mut().enumerate() {
            let nibble = (self.nonce >> (60 - 4 * i)) as u8 & 0xf;
            *slot = b"0123456789abcdef"[usize::from(nibble)];
        }
        out
    }

    /// Accept `signed` if its MAC is right and it names this session's nonce
    /// with a `seq` above the last accepted one; `nonce` and `seq` come from
    /// parsing the signed prefix. Nothing changes on refusal.
    pub fn verify(&mut self, signed: &Signed, nonce: &str, seq: u32) -> bool {
        let mac = hmac_sha256(&self.key, &[signed.prefix]);
        let fresh = nonce.as_bytes() == self.nonce_hex() && seq > self.last_seq;
        if same(&mac, &signed.mac) && fresh {
            self.last_seq = seq;
            true
        } else {
            false
        }
    }
}
