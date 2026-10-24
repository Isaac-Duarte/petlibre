//! Base64 against RFC 4648's test vectors.

use petlibre_core::base64::{encode, encoded_len};

#[test]
fn rfc_4648_vectors() {
    for (plain, text) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        let mut out = [0; 8];
        assert_eq!(encode(plain.as_bytes(), &mut out), Some(text));
    }
    let mut out = [0; 4];
    assert_eq!(encode(&[0xff, 0xef, 0xbe], &mut out), Some("/+++"));
    assert_eq!(encode(b"abcd", &mut out), None, "needs 8 bytes");
    assert_eq!(encoded_len(120), 160);
}
