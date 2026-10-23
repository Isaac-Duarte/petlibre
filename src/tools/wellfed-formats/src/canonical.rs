//! Hashes and the canonical JSON the former Python tools also wrote.
//!
//! Canonical form matches Python's `json.dumps(value, sort_keys=True,
//! separators=(",", ":"), allow_nan=False)` byte for byte, so journals and
//! plans the Python tools wrote still check. Plan digests are taken
//! over these bytes and bindings are compared in this form, so any change to
//! the encoding breaks agreement with records already written.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Error, Result, hex};

pub fn sha256(data: &[u8]) -> String {
    hex::encode(&Sha256::digest(data))
}

/// Canonical bytes; only integers are accepted as numbers, since no plan,
/// journal or binding here carries a float.
pub fn canonical(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

/// Canonical bytes of a typed value: the same bytes as for its `Value`.
pub fn canonical_of<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    canonical(&serde_json::to_value(value)?)
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => {
            if !(number.is_i64() || number.is_u64()) {
                return Err(Error::NotInteger);
            }
            out.extend_from_slice(number.to_string().as_bytes());
        }
        Value::String(text) => string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // Code-point order of the keys, as Python's sort_keys gives.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                string(key, out);
                out.push(b':');
                write(&map[key], out)?;
            }
            out.push(b'}');
        }
    }

    Ok(())
}

/// Python's default `ensure_ascii` escaping of a JSON string, quotes included.
pub fn string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            ' '..='~' => out.push(c as u8),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
                }
            }
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_python_sort_keys_compact_ascii() {
        let value = json!({"b": [1, -2, null, true], "a": "é\"\n\u{1}😀", "A": {}});
        assert_eq!(
            String::from_utf8(canonical(&value).unwrap()).unwrap(),
            "{\"A\":{},\"a\":\"\\u00e9\\\"\\n\\u0001\\ud83d\\ude00\",\"b\":[1,-2,null,true]}"
        );
    }

    #[test]
    fn delete_is_escaped_like_python() {
        assert_eq!(canonical(&json!("\u{7f}~")).unwrap(), b"\"\\u007f~\"");
    }

    #[test]
    fn floats_are_refused() {
        assert!(canonical(&json!({"x": 1.5})).is_err());
    }

    #[test]
    fn digest_is_lowercase_hex() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(hex::is_sha256(&sha256(b"x")));
        assert!(!hex::is_sha256(&sha256(b"x").to_uppercase()));
    }
}
