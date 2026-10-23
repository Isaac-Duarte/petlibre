//! `src/firmware/devices/<name>/device.json`: one board the builder targets.
//!
//! A device names its platform (the chip family whose SDK, overlays and
//! audits are code in the builder) and the board-level files that platform
//! takes: the application entry and first-boot mains it stages, the
//! Bluetooth source a firmware may opt into, and the verified stock image and layout the device profiles derive their flash
//! banks and pad table from. Every path is root-relative, and the builder
//! binds every one a profile reads into its identity.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub schema_version: u32,
    /// The product's model name.
    pub model: String,
    /// Absent until the board's stock image is verified; the device and
    /// first-boot profiles refuse without it.
    pub stock: Option<Stock>,
    pub platform: PlatformConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stock {
    /// The verified stock OTA image (ignored private evidence).
    pub image: String,
    /// Its verified flash layout ([`crate::layout::Layout`]).
    pub layout: String,
}

/// The chip family, with the board-level files only that platform takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PlatformConfig {
    /// Realtek AmebaD (RTL8721D): a KM0 and a KM4 core.
    Amebad {
        /// The SDK `example_entry.c` replacement every Rust profile stages.
        entry: String,
        /// The UART-only SDK mains the first-boot profile stages.
        firstboot_mains: AmebadMains,
        /// The BLE peripheral beside the entry, staged for a firmware that
        /// declares `bluetooth = true`; absent on a board without it.
        #[serde(default)]
        bluetooth: Option<String>,
    },
}

impl PlatformConfig {
    /// The platform's name, as `wellfed-build list` prints it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Amebad { .. } => "amebad",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmebadMains {
    pub km0: String,
    pub km4: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_devices_load() {
        let devices = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../src/firmware/devices");
        let mut loaded = 0;
        for entry in std::fs::read_dir(devices).unwrap() {
            let path = entry.unwrap().path().join("device.json");
            let device: DeviceConfig = crate::load(&path).unwrap();
            assert_eq!(device.schema_version, 1);
            loaded += 1;
        }
        assert!(loaded > 0);
    }

    #[test]
    fn the_platform_is_tagged() {
        let json = r#"{"schema_version": 1, "model": "X", "stock": null,
            "platform": {"kind": "amebad", "entry": "e.c",
                         "firstboot_mains": {"km0": "a.c", "km4": "b.c"}}}"#;
        let device: DeviceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(device.platform.name(), "amebad");

        let unknown = json.replace("\"amebad\"", "\"esp32\"");
        assert!(serde_json::from_str::<DeviceConfig>(&unknown).is_err());
    }
}
