//! HMAC-SHA256 (RFC 2104) and a constant-time compare: the key schedule of
//! Noise, the generator's DRBG, the setup code's commitments and the
//! speaker variant's signed stream share them.

use sha2::{Digest, Sha256};

pub const KEY_LEN: usize = 32;

/// HMAC-SHA256 of the concatenated `parts`.
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

/// Equal without an early exit, so timing does not reveal a partial match.
pub(crate) fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
