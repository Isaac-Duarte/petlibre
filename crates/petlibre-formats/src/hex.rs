//! Lowercase hex, the form digests, keys and identities are written in.

pub fn encode(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Exactly `N` bytes of hex, either case.
pub fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = decode_vec(text)?;
    bytes.try_into().ok()
}

/// Any whole number of bytes of hex, either case.
pub fn decode_vec(text: &str) -> Option<Vec<u8>> {
    // `from_str_radix` alone would also take a leading `+`.
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

/// `len` lowercase hex digits: keys and identities are compared as written.
pub fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Lowercase only, the form [`crate::canonical::sha256`] writes, so digests
/// compare as plain strings.
pub fn is_sha256(value: &str) -> bool {
    is_lower_hex(value, 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(encode(&[0x00, 0xab, 0xff]), "00abff");
        assert_eq!(decode::<3>("00ABff"), Some([0x00, 0xab, 0xff]));
        assert_eq!(decode_vec(""), Some(vec![]));
    }

    #[test]
    fn refuses_bad_lengths_and_digits() {
        assert_eq!(decode::<2>("00ab00"), None);
        assert_eq!(decode_vec("abc"), None);
        assert_eq!(decode_vec("zz"), None);
        assert_eq!(decode_vec("+1"), None);
        assert_eq!(decode_vec("é0"), None);
    }
}
