//! Strict lowercase hex, the only form the firmware's text fields accept.
//!
//! Keys, MACs, nonces and Wi-Fi credentials all arrive as lowercase hex
//! digits; an uppercase digit or any other byte is refused rather than
//! normalized, so a request has exactly one accepted spelling.

/// One lowercase hex digit's value.
#[inline]
pub const fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Every byte is a lowercase hex digit (true for an empty input).
pub fn is_lower(text: &[u8]) -> bool {
    text.iter().all(|&b| nibble(b).is_some())
}

/// Decode `hex` into the start of `out` and return the byte count; `None`
/// for an odd length, more bytes than `out` holds, or a non-digit. `out` may
/// be partly written on failure.
pub fn decode(hex: &[u8], out: &mut [u8]) -> Option<usize> {
    if !hex.len().is_multiple_of(2) || hex.len() / 2 > out.len() {
        return None;
    }

    for (byte, pair) in out.iter_mut().zip(hex.as_chunks::<2>().0) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(hex.len() / 2)
}

/// Exactly `2 * N` lowercase hex digits as `N` bytes.
pub fn decode_array<const N: usize>(hex: &[u8]) -> Option<[u8; N]> {
    let mut out = [0u8; N];
    (hex.len() == 2 * N && decode(hex, &mut out).is_some()).then_some(out)
}

/// Bytes as lowercase hex when formatted, for record fields.
#[derive(Clone, Copy, Debug)]
pub struct Lower<'a>(pub &'a [u8]);

impl core::fmt::Display for Lower<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}
