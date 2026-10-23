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

use std::{fmt::Write as _, path::Path};

use anyhow::Result;
use wellfed_core::request_auth::hmac_sha256;
use wellfed_formats::hex;
use uuid::Uuid;

use crate::storage;

/// The firmware's request limit (wellfed-core `MAX_REQUEST`), raised from
/// 256 for signed `time` requests (about 260 bytes) and again for signed
/// `wifi` requests with hex credentials (up to 383 bytes).
pub const MAX_REQUEST: usize = 448;
/// Hex characters in a request key.
pub const KEY_HEX_LEN: usize = 64;
/// Hex characters in a health nonce.
pub const NONCE_HEX_LEN: usize = 16;
/// What separates the signed prefix from the MAC.
pub const MAC_FIELD: &str = r#","mac":""#;

/// Whether `text` is a request key as stored: 64 lowercase hex characters.
pub fn valid_key_hex(text: &str) -> bool {
    hex::is_lower_hex(text, KEY_HEX_LEN)
}

/// Whether `text` is a health nonce: 16 lowercase hex characters.
pub fn valid_nonce(text: &str) -> bool {
    hex::is_lower_hex(text, NONCE_HEX_LEN)
}

// Intentionally no Debug: this is the firmware's request-signing secret.
#[derive(Clone)]
pub struct RequestKey([u8; 32]);

impl RequestKey {
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn parse(text: &str) -> Option<Self> {
        valid_key_hex(text)
            .then(|| hex::decode(text))
            .flatten()
            .map(Self)
    }

    /// 32 bytes from the operating system's CSPRNG.
    pub fn generate() -> Result<Self> {
        storage::random(Path::new(storage::SYSTEM_RANDOM)).map(Self)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    pub fn mac(&self, message: &[u8]) -> [u8; 32] {
        hmac_sha256(&self.0, &[message])
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
        let _ = write!(self.0, r#","{name}":"{}""#, hex::encode(bytes));
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
        let mac = hex::encode(&key.mac(text.as_bytes()));
        text + MAC_FIELD + &mac + "\"}"
    }
}

/// Split a signed request into the MAC'd prefix and the MAC hex; `None` if
/// it does not end in the signed layout. For tests and diagnostics.
pub fn split_signed(payload: &str) -> Option<(&str, &str)> {
    let at = payload.rfind(MAC_FIELD)?;
    let mac = payload[at + MAC_FIELD.len()..].strip_suffix("\"}")?;
    hex::is_lower_hex(mac, 64).then_some((&payload[..at], mac))
}

/// Whether `payload` carries a valid MAC under `key`. For tests and
/// diagnostics; the firmware does its own check.
pub fn verify(key: &RequestKey, payload: &str) -> bool {
    use subtle::ConstantTimeEq;
    split_signed(payload).is_some_and(|(prefix, mac)| {
        hex::encode(&key.mac(prefix.as_bytes()))
            .as_bytes()
            .ct_eq(mac.as_bytes())
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(storage::random::<32>(Path::new("/nonexistent/random")).is_err());
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
        assert_eq!(
            mac,
            hex::encode(&hmac_sha256(&[0xab; 32], &[prefix.as_bytes()]))
        );
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
