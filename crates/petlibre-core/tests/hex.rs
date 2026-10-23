//! The shared strict decoder: lowercase only, exact lengths, no partial
//! acceptance.

use petlibre_core::hex::{decode, decode_array, is_lower, nibble};

#[test]
fn nibbles_are_lowercase_digits_only() {
    for c in 0..=u8::MAX {
        let expected = match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        assert_eq!(nibble(c), expected, "{c:#x}");
    }
}

#[test]
fn decode_fills_the_start_and_reports_the_length() {
    let mut out = [0xee; 4];
    assert_eq!(decode(b"00a5ff", &mut out), Some(3));
    assert_eq!(out, [0x00, 0xa5, 0xff, 0xee]);

    assert_eq!(decode(b"", &mut out), Some(0));
    assert_eq!(decode(b"0", &mut out), None);
    assert_eq!(decode(b"0000000000", &mut out), None);
    assert_eq!(decode(b"0A", &mut out), None);
    assert_eq!(decode(b"0g", &mut out), None);
}

#[test]
fn arrays_need_the_exact_length() {
    assert_eq!(decode_array::<2>(b"a5b6"), Some([0xa5, 0xb6]));
    assert_eq!(decode_array::<2>(b"a5"), None);
    assert_eq!(decode_array::<2>(b"a5b6c7"), None);
    assert_eq!(decode_array::<2>(b"A5B6"), None);
}

#[test]
fn is_lower_accepts_only_lowercase_digits() {
    assert!(is_lower(b""));
    assert!(is_lower(b"0123456789abcdef"));
    assert!(!is_lower(b"0123456789ABCDEF"));
    assert!(!is_lower(b"12 4"));
}
