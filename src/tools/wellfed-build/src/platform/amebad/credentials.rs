//! The generated credential header: the Wi-Fi join, read from an ignored
//! private file into an untracked SDK header.
//!
//! Only the private file's hash reaches the manifest (it is an identity
//! input). Values are octal-escaped C strings, so no SSID or password can
//! break out of its literal.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use wellfed_formats::credentials::WifiCredential;

use wellfed_formats::require;

use crate::Result;

/// Why the private Wi-Fi file was refused. Neither variant carries a value
/// from the file: it holds the network's passphrase.
#[derive(Debug, thiserror::Error)]
pub enum CredentialsError {
    #[error("Wi-Fi credentials need string ssid and password")]
    Shape,
    #[error("Wi-Fi credentials need a 1-32 byte SSID and an 8-63 byte passphrase")]
    Size,
}

const BANNER: &str = "/* Generated from a private file by wellfed-build; never tracked. */\n";

/// Octal-escape everything but plain printable ASCII; never a trigraph or
/// quote break.
fn c_string(value: &[u8]) -> String {
    let mut out = String::from("\"");
    for &byte in value {
        if (0x20..0x7f).contains(&byte) && !b"\"\\?".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("\\{byte:03o}"));
        }
    }
    out.push('"');
    out
}

/// A credential file, which must be an object: derived structs would also
/// accept their fields as an array.
fn object<T: DeserializeOwned>(raw: &[u8]) -> serde_json::Result<T> {
    let map: Map<String, Value> = serde_json::from_slice(raw)?;
    serde_json::from_value(Value::Object(map))
}

/// The Wi-Fi header; refuses anything but a WPA2-sized passphrase.
pub(crate) fn wifi_header(raw: &[u8]) -> Result<String> {
    let WifiCredential { ssid, password } = object(raw).map_err(|_| CredentialsError::Shape)?;
    require(
        (1..=32).contains(&ssid.len()) && (8..=63).contains(&password.len()),
        || CredentialsError::Size,
    )?;

    Ok(format!(
        "{BANNER}#define WELLFED_WIFI_SSID {}\n#define WELLFED_WIFI_PASSWORD {}\n",
        c_string(ssid.as_bytes()),
        c_string(password.as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_values_are_escaped_and_sized() {
        let text = wifi_header(br#"{"ssid": "lab\"?", "password": "pass\\word"}"#).unwrap();
        assert!(text.contains("#define WELLFED_WIFI_SSID \"lab\\042\\077\"\n"));
        assert!(text.contains("#define WELLFED_WIFI_PASSWORD \"pass\\134word\"\n"));

        for bad in [
            r#"{"ssid": "", "password": "longenough"}"#,
            r#"{"ssid": "lab", "password": "short"}"#,
        ] {
            assert!(
                matches!(
                    wifi_header(bad.as_bytes()),
                    Err(crate::Error::Credentials(CredentialsError::Size))
                ),
                "{bad}"
            );
        }
        for bad in [
            r#"{"ssid": 1, "password": "longenough"}"#,
            r#"["lab", "longenough"]"#,
        ] {
            assert!(
                matches!(
                    wifi_header(bad.as_bytes()),
                    Err(crate::Error::Credentials(CredentialsError::Shape))
                ),
                "{bad}"
            );
        }
    }
}
