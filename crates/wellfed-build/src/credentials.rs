//! The generated credential headers: FW07 Wi-Fi join and FW08 broker
//! settings, read from ignored private files into untracked SDK headers.
//!
//! Only the private files' hashes reach the manifest (they are identity
//! inputs). Values are octal-escaped C strings, so no SSID or password can
//! break out of its literal.

use std::net::Ipv4Addr;

use serde_json::{Map, Value};

use crate::identity::is_lower_hex;
use crate::{Error, Result, require};

const BANNER: &str = "/* Generated from a private file by wellfed-build; never tracked. */\n";

/// Octal-escape everything but plain printable ASCII; never a trigraph or
/// quote break.
pub fn c_string(value: &[u8]) -> String {
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

fn object(raw: &[u8]) -> Result<Map<String, Value>> {
    match serde_json::from_slice(raw)? {
        Value::Object(map) => Ok(map),
        _ => Err(Error::Refused(
            "Credentials file is not a JSON object".into(),
        )),
    }
}

/// The Wi-Fi header; refuses anything but a WPA2-sized passphrase.
pub fn wifi_header(raw: &[u8]) -> Result<String> {
    let config = object(raw)?;
    let (Some(ssid), Some(password)) = (
        config.get("ssid").and_then(Value::as_str),
        config.get("password").and_then(Value::as_str),
    ) else {
        return Err(Error::Refused(
            "Wi-Fi credentials need string ssid and password".into(),
        ));
    };
    require(
        (1..=32).contains(&ssid.len()) && (8..=63).contains(&password.len()),
        "Wi-Fi credentials need a 1-32 byte SSID and an 8-63 byte passphrase",
    )?;

    Ok(format!(
        "{BANNER}#define WELLFED_WIFI_SSID {}\n#define WELLFED_WIFI_PASSWORD {}\n",
        c_string(ssid.as_bytes()),
        c_string(password.as_bytes())
    ))
}

/// The broker settings header; refuses anything `wellfed-local
/// firmware-credential` would not have written.
pub fn transport_header(raw: &[u8]) -> Result<String> {
    const FIELDS: [&str; 8] = [
        "version",
        "broker",
        "port",
        "device_id",
        "client_id",
        "username",
        "password",
        "namespace",
    ];
    let config = object(raw)?;
    // `request_key` (wellfed-local firmware-request-key) is optional.
    require(
        FIELDS.iter().all(|field| config.contains_key(*field))
            && config
                .keys()
                .all(|key| FIELDS.contains(&key.as_str()) || key == "request_key")
            && config["version"].as_u64() == Some(1),
        "Transport credentials need exactly the version 1 fields",
    )?;
    let text = |key: &str| config[key].as_str().unwrap_or_default();

    let request_key = match config.get("request_key") {
        None | Some(Value::Null) => "",
        Some(Value::String(key)) => key,
        Some(_) => "-",
    };
    require(
        request_key.is_empty() || is_lower_hex(request_key, 64),
        "Transport request key must be 64 lowercase hex characters",
    )?;

    let device = text("device_id");
    require(
        (1..=64).contains(&device.len())
            && device
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Transport credentials need a plain device ID",
    )?;

    let broker: Ipv4Addr = text("broker")
        .parse()
        .map_err(|_| Error::Refused("Transport broker is not an IPv4 address".into()))?;
    require(
        !broker.is_unspecified() && !broker.is_multicast() && !broker.is_broadcast(),
        "Transport broker must be a unicast IPv4 address",
    )?;

    let port = config["port"]
        .as_u64()
        .filter(|port| (1..=65535).contains(port));
    let Some(port) = port else {
        return Err(Error::Refused("Transport port out of range".into()));
    };
    require(
        text("client_id") == format!("wellfed-fw-{device}")
            && text("username") == "wellfed-fw"
            && text("namespace") == format!("wellfed/1/{device}/"),
        "Transport identity does not match the device ID",
    )?;

    let password = text("password");
    require(
        (32..=128).contains(&password.len())
            && password
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "Transport password must be 32-128 lowercase hex characters",
    )?;

    let mut out = String::from(BANNER);
    let mut define = |key: &str, value: String| {
        out.push_str(&format!("#define WELLFED_MQTT_{key} {value}\n"));
    };
    define("BROKER", format!("0x{:08x}U", u32::from(broker)));
    define("PORT", format!("{port}U"));
    for key in ["client_id", "username", "password", "namespace"] {
        define(&key.to_uppercase(), c_string(text(key).as_bytes()));
    }
    define("REQUEST_KEY", c_string(request_key.as_bytes()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn good() -> Value {
        json!({"version": 1, "broker": "10.42.103.1", "port": 1883, "device_id": "dev1",
               "client_id": "wellfed-fw-dev1", "username": "wellfed-fw",
               "password": "0123456789abcdef".repeat(4), "namespace": "wellfed/1/dev1/"})
    }

    fn header(change: Value) -> Result<String> {
        let mut config = good();
        for (key, value) in change.as_object().unwrap() {
            config[key] = value.clone();
        }
        transport_header(config.to_string().as_bytes())
    }

    #[test]
    fn good_credentials_become_defines() {
        let text = header(json!({})).unwrap();
        assert!(text.contains("#define WELLFED_MQTT_BROKER 0x0a2a6701U\n"));
        assert!(text.contains("#define WELLFED_MQTT_PORT 1883U\n"));
        assert!(text.contains("#define WELLFED_MQTT_CLIENT_ID \"wellfed-fw-dev1\"\n"));
        assert!(text.contains("#define WELLFED_MQTT_NAMESPACE \"wellfed/1/dev1/\"\n"));
        assert!(text.contains(&format!(
            "#define WELLFED_MQTT_PASSWORD \"{}\"\n",
            "0123456789abcdef".repeat(4)
        )));
        assert!(text.contains("#define WELLFED_MQTT_REQUEST_KEY \"\"\n"));

        let keyed = header(json!({"request_key": "ab".repeat(32)})).unwrap();
        assert!(keyed.contains(&format!("REQUEST_KEY \"{}\"", "ab".repeat(32))));
    }

    #[test]
    fn anything_the_server_would_not_write_is_refused() {
        for change in [
            json!({"version": 2}),
            json!({"extra": 1}),
            json!({"broker": "0.0.0.0"}),
            json!({"broker": "224.0.0.1"}),
            json!({"broker": "255.255.255.255"}),
            json!({"port": 0}),
            json!({"port": "1883"}),
            json!({"port": true}),
            json!({"client_id": "other"}),
            json!({"username": "device"}),
            json!({"namespace": "dl/PLAF103/dev1/"}),
            json!({"device_id": "dev/1"}),
            json!({"password": "short"}),
            json!({"password": "X".repeat(64)}),
            json!({"password": format!("\"{}", "a".repeat(63))}),
            json!({"request_key": "AB".repeat(32)}),
        ] {
            assert!(header(change.clone()).is_err(), "{change}");
        }

        let mut missing = good();
        missing.as_object_mut().unwrap().remove("port");
        assert!(transport_header(missing.to_string().as_bytes()).is_err());
    }

    #[test]
    fn wifi_values_are_escaped_and_sized() {
        let text = wifi_header(br#"{"ssid": "lab\"?", "password": "pass\\word"}"#).unwrap();
        assert!(text.contains("#define WELLFED_WIFI_SSID \"lab\\042\\077\"\n"));
        assert!(text.contains("#define WELLFED_WIFI_PASSWORD \"pass\\134word\"\n"));

        for bad in [
            r#"{"ssid": "", "password": "longenough"}"#,
            r#"{"ssid": "lab", "password": "short"}"#,
            r#"{"ssid": 1, "password": "longenough"}"#,
            r#"["lab", "longenough"]"#,
        ] {
            assert!(wifi_header(bad.as_bytes()).is_err(), "{bad}");
        }
    }
}
