//! Standard base64 (RFC 4648, padded), for binary in text records: the
//! raw noise samples travel over UART this way, a third denser than hex.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The encoded length of `len` bytes.
pub const fn encoded_len(len: usize) -> usize {
    len.div_ceil(3) * 4
}

/// Encode `bytes` into `out`, returning the text, or `None` if `out` is
/// shorter than [`encoded_len`].
pub fn encode<'a>(bytes: &[u8], out: &'a mut [u8]) -> Option<&'a str> {
    let out = out.get_mut(..encoded_len(bytes.len()))?;
    for (chunk, text) in bytes.chunks(3).zip(out.chunks_mut(4)) {
        let word = chunk
            .iter()
            .enumerate()
            .fold(0u32, |word, (i, &b)| word | u32::from(b) << (16 - 8 * i));
        for (i, slot) in text.iter_mut().enumerate() {
            *slot = if i <= chunk.len() {
                let index = (word >> (18 - 6 * i)) & 0x3f;
                ALPHABET[index as usize]
            } else {
                b'='
            };
        }
    }
    core::str::from_utf8(out).ok()
}
