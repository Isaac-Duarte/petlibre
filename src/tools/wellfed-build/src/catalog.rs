//! `wellfed-build list`: the devices and firmwares this checkout can build,
//! with each firmware's variants, for people and scripts choosing a build.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Catalog {
    pub devices: Vec<DeviceEntry>,
    pub firmwares: Vec<FirmwareEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceEntry {
    pub name: String,
    pub model: String,
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FirmwareEntry {
    pub name: String,
    pub devices: Vec<String>,
    /// Links the device's Bluetooth stack.
    pub bluetooth: bool,
    pub variants: Vec<VariantEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VariantEntry {
    pub name: String,
    pub summary: String,
    /// Drives the motor: its policies name a motion policy.
    pub motion: bool,
}
