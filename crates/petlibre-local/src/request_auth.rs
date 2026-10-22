//! Authenticated server-to-firmware requests.
//!
//! A replacement firmware built with a request key reports a fresh 64-bit
//! `nonce` in each MQTT session's health. The server then appends that nonce,
//! a per-nonce `seq` starting at 1, and an HMAC-SHA256 to every request:
//!
//! ```text
//! {"id":"<uuid>","op":"<op>"[,<fields>],"nonce":"<16 hex>","seq":<n>,"mac":"<64 hex>"}
//! ```
//!
//! The MAC covers every byte before the literal `,"mac":"`, so the request is
//! built here as text in a fixed field order rather than through a JSON map.
//! The key is secret: [`RequestKey`] has no `Debug` and is never logged.

use std::{fmt::Write as _, fs, io::Read, path::Path};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The firmware's request limit (petlibre-core `MAX_REQUEST`), raised from
/// 256 for signed `time` requests (about 260 bytes) and again for signed
/// `wifi` requests with hex credentials (up to 383 bytes).
pub const MAX_REQUEST: usize = 448;
/// Hex characters in a request key.
pub const KEY_HEX_LEN: usize = 64;
/// Hex characters in a health nonce.
pub const NONCE_HEX_LEN: usize = 16;
/// What separates the signed prefix from the MAC.
pub const MAC_FIELD: &str = r#","mac":""#;

const BLOCK: usize = 64;

/// HMAC-SHA256 (RFC 2104) over `sha2`, so no further dependency is needed.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `text` is a request key as stored: 64 lowercase hex characters.
pub fn valid_key_hex(text: &str) -> bool {
    lower_hex(text, KEY_HEX_LEN)
}

/// Whether `text` is a health nonce: 16 lowercase hex characters.
pub fn valid_nonce(text: &str) -> bool {
    lower_hex(text, NONCE_HEX_LEN)
}

// Intentionally no Debug: this is the firmware's request-signing secret.
#[derive(Clone)]
pub struct RequestKey([u8; 32]);

impl RequestKey {
    pub fn parse(text: &str) -> Option<Self> {
        if !valid_key_hex(text) {
            return None;
        }
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Self(key))
    }

    /// 32 bytes from the operating system's CSPRNG (`/dev/urandom`, which
    /// the kernel seeds before userspace can run this command).
    pub fn generate() -> Result<Self> {
        Self::generate_from(Path::new("/dev/urandom"))
    }

    fn generate_from(source: &Path) -> Result<Self> {
        let mut key = [0u8; 32];
        fs::File::open(source)
            .and_then(|mut file| file.read_exact(&mut key))
            .context("read the system random source")?;
        Ok(Self(key))
    }

    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    pub fn mac(&self, message: &[u8]) -> [u8; 32] {
        hmac_sha256(&self.0, message)
    }
}

/// A request object being built, without its closing brace.
pub struct Request(String);

impl Request {
    pub fn new(id: Uuid, op: &str) -> Self {
        Self(format!(r#"{{"id":"{id}","op":"{op}""#))
    }

    /// Append a lowercase-hex string field (FW25 credentials); names are
    /// fixed identifiers from this crate.
    pub fn hex_field(mut self, name: &str, bytes: &[u8]) -> Self {
        let _ = write!(self.0, r#","{name}":"{}""#, hex(bytes));
        self
    }

    /// Append a string field whose value is a fixed identifier, such as a
    /// built-in tune's name; never text from a client.
    pub fn name_field(mut self, name: &str, value: &'static str) -> Self {
        let _ = write!(self.0, r#","{name}":"{value}""#);
        self
    }

    /// Append an integer field; names are fixed identifiers from this crate.
    pub fn field(mut self, name: &str, value: i64) -> Self {
        let _ = write!(self.0, r#","{name}":{value}"#);
        self
    }

    pub fn unsigned(self) -> String {
        self.0 + "}"
    }

    /// Append `nonce` and `seq`, then the MAC over everything before it.
    pub fn signed(self, key: &RequestKey, nonce: &str, seq: u32) -> String {
        let mut text = self.0;
        let _ = write!(text, r#","nonce":"{nonce}","seq":{seq}"#);
        let mac = hex(&key.mac(text.as_bytes()));
        text + MAC_FIELD + &mac + "\"}"
    }
}

/// Split a signed request into the MAC'd prefix and the MAC hex; `None` if
/// it does not end in the signed layout. For tests and diagnostics.
pub fn split_signed(payload: &str) -> Option<(&str, &str)> {
    let at = payload.rfind(MAC_FIELD)?;
    let mac = payload[at + MAC_FIELD.len()..].strip_suffix("\"}")?;
    lower_hex(mac, 64).then_some((&payload[..at], mac))
}

/// Whether `payload` carries a valid MAC under `key`. For tests and
/// diagnostics; the firmware does its own check.
pub fn verify(key: &RequestKey, payload: &str) -> bool {
    use subtle::ConstantTimeEq;
    split_signed(payload).is_some_and(|(prefix, mac)| {
        hex(&key.mac(prefix.as_bytes()))
            .as_bytes()
            .ct_eq(mac.as_bytes())
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 1.
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block is hashed first.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn keys_and_nonces_are_lowercase_hex_of_a_fixed_length() {
        let text = "0123456789abcdef".repeat(4);
        assert_eq!(RequestKey::parse(&text).unwrap().to_hex(), text);
        for bad in [
            text.to_uppercase(),
            text[1..].to_owned(),
            format!("{text}0"),
            "g".repeat(64),
        ] {
            assert!(RequestKey::parse(&bad).is_none(), "{bad}");
        }
        assert!(valid_nonce("0123456789abcdef"));
        assert!(!valid_nonce("0123456789ABCDEF"));
        assert!(!valid_nonce("0123456789abcde"));
    }

    #[test]
    fn generated_keys_differ() {
        let a = RequestKey::generate().unwrap().to_hex();
        let b = RequestKey::generate().unwrap().to_hex();
        assert!(valid_key_hex(&a) && a != b);
        assert!(RequestKey::generate_from(Path::new("/nonexistent/random")).is_err());
    }

    #[test]
    fn a_signed_request_keeps_the_field_order_and_its_mac_covers_the_prefix() {
        let key = RequestKey::parse(&"ab".repeat(32)).unwrap();
        let id = Uuid::nil();
        let request = Request::new(id, "ping");
        let signed = Request::new(id, "ping").signed(&key, "0123456789abcdef", 7);

        assert_eq!(
            request.unsigned(),
            format!(r#"{{"id":"{id}","op":"ping"}}"#)
        );
        let (prefix, mac) = split_signed(&signed).unwrap();
        assert_eq!(
            prefix,
            format!(r#"{{"id":"{id}","op":"ping","nonce":"0123456789abcdef","seq":7"#)
        );
        assert_eq!(mac, hex(&hmac_sha256(&[0xab; 32], prefix.as_bytes())));
        assert!(verify(&key, &signed));
        assert!(!signed.contains(char::is_whitespace));

        let value: serde_json::Value = serde_json::from_str(&signed).unwrap();
        assert_eq!(value["seq"], 7);

        // Any change to the prefix breaks the MAC.
        assert!(!verify(&key, &signed.replace("\"seq\":7", "\"seq\":8")));
        let other = RequestKey::parse(&"cd".repeat(32)).unwrap();
        assert!(!verify(&other, &signed));
    }
}
