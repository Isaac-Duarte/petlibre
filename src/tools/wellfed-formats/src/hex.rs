//! Lowercase hex, the form digests, keys and identities are written in.
//!
//! The digits are the session crate's, the ones the feeder and its clients
//! use; this adds the owned forms the host tools want, and a decoder that
//! also takes uppercase, for text the tools did not write (an eFuse dump, a
//! MAC address).

use wellfed_session::hex::{self as digits, Lower};

pub fn encode(data: &[u8]) -> String {
    Lower(data).to_string()
}

/// Exactly `N` bytes of hex, either case.
pub fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    digits::decode_array(text.to_ascii_lowercase().as_bytes())
}

/// Any whole number of bytes of hex, either case.
pub fn decode_vec(text: &str) -> Option<Vec<u8>> {
    let mut out = vec![0; text.len() / 2];
    digits::decode(text.to_ascii_lowercase().as_bytes(), &mut out)?;
    Some(out)
}

/// `len` lowercase hex digits: keys and identities are compared as written.
pub fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len && digits::is_lower(value.as_bytes())
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
