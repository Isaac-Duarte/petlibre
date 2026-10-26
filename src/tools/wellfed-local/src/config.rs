//! The private server configuration (`private/server.json`, format version
//! 1), which is stock's (`wellfed_stock::config`).
//!
//! A file written while the replacement firmware still had a broker role
//! keeps that role's `firmware` field. Loading drops it unread, so such a
//! file still loads without being rewritten (it holds working credentials
//! and is edited only by hand, with the service stopped); stock's own type
//! still refuses any other field it does not know.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};

pub use wellfed_stock::config::{Config, ImportOptions, import};

/// The retired replacement-firmware broker role's field.
const RETIRED_ROLE: &str = "firmware";

pub fn load(path: &Path) -> Result<Config> {
    let bytes = fs::read(path).context("read configuration")?;
    let config = parse(&bytes).context("invalid server configuration")?;
    config.validate()?;
    Ok(config)
}

/// The retired role's field off the top, then the rest as stock's. A
/// repeated key is refused, as a struct parse of the whole file would.
pub(crate) fn parse(bytes: &[u8]) -> serde_json::Result<Config> {
    let UniqueKeys(mut fields) = serde_json::from_slice(bytes)?;
    fields.remove(RETIRED_ROLE);
    serde_json::from_value(Value::Object(fields))
}

/// A JSON object whose keys each appear once.
struct UniqueKeys(Map<String, Value>);

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueKeys;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<UniqueKeys, A::Error> {
                let mut fields = Map::new();
                while let Some((key, value)) = access.next_entry::<String, Value>()? {
                    if fields.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("duplicate field `{key}`")));
                    }
                    fields.insert(key, value);
                }
                Ok(UniqueKeys(fields))
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = r#""version":1,"device_id":"AF_TEST","device_client_id":"AF_TEST",
        "device_username":"dev","device_password":"pw",
        "server_password":"ssssssssssssssssssssssssssssssss",
        "api_token":"tttttttttttttttttttttttttttttttt",
        "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765",
        "timezone":"UTC","plans":[],"binding_response":null,
        "hotspot_interface":"wlan1","target_mac":"02:00:00:00:00:01""#;

    const FIRMWARE: &str = r#""firmware":{"client_id":"wellfed-fw-AF_TEST",
        "username":"wellfed-fw","request_key":null,
        "password":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

    #[test]
    fn a_file_with_the_retired_role_still_loads() {
        let config = parse(format!("{{{STOCK},{FIRMWARE}}}").as_bytes()).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.device_id, "AF_TEST");
    }

    #[test]
    fn a_repeated_key_is_refused_as_the_struct_parse_did() {
        let firmware_twice = format!("{{{STOCK},{FIRMWARE},{FIRMWARE}}}");
        let error = parse(firmware_twice.as_bytes()).err().unwrap();
        assert!(error.to_string().contains("duplicate field `firmware`"));

        let version_twice = format!("{{{STOCK},\"version\":1}}");
        let error = parse(version_twice.as_bytes()).err().unwrap();
        assert!(error.to_string().contains("duplicate field `version`"));
    }

    #[test]
    fn any_other_unknown_field_is_still_refused() {
        let extra = format!("{{{STOCK},\"other\":1}}");
        assert!(parse(extra.as_bytes()).is_err());
    }
}
