//! `src/firmware/vendor/plaf103-layout.json`: the verified stock flash layout the
//! builder points the SDK's banks at and the audits check images against.

use std::fmt;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

/// An address or offset the layout writes as a `0x…` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hex(pub u64);

impl Serialize for Hex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("0x{:08x}", self.0))
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let digits = text
            .strip_prefix("0x")
            .or_else(|| text.strip_prefix("0X"))
            .unwrap_or(&text);
        u64::from_str_radix(digits, 16)
            .map(Hex)
            .map_err(|_| de::Error::custom(format_args!("not a hex number: {text}")))
    }
}

impl fmt::Display for Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:08x}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub schema_version: u32,
    pub model: String,
    pub stock_version: String,
    pub stock_bytes: u64,
    pub stock_sha256: String,
    pub flash_mapping_base: Hex,
    pub sector_bytes: u64,
    pub ota_bank_offsets: Vec<Hex>,
    pub candidate_payload_cap_bytes: u64,
    pub bank_table_runtime_address: Hex,
    pub bank_selector_entry: Hex,
    pub known_preserve_ranges: Vec<PreserveRange>,
    pub complete_partition_map: bool,
    pub flash_capacity_verified: bool,
    pub boot_acceptance_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreserveRange {
    pub name: String,
    pub start: Hex,
    pub end_exclusive: Hex,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_layout_loads() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../src/firmware/vendor/plaf103-layout.json"
        );
        let layout: Layout = crate::load(std::path::Path::new(path)).unwrap();
        assert_eq!(layout.schema_version, 1);
        assert_eq!(layout.flash_mapping_base, Hex(0x0800_0000));
        assert_eq!(layout.ota_bank_offsets, [Hex(0x6000), Hex(0x18_6000)]);
        assert!(!layout.known_preserve_ranges.is_empty());
    }

    #[test]
    fn hex_takes_either_prefix_and_refuses_decimal_numbers() {
        let parse = |json: &str| serde_json::from_str::<Hex>(json).ok();
        assert_eq!(parse("\"0X1f\""), Some(Hex(0x1f)));
        assert_eq!(parse("\"1f\""), Some(Hex(0x1f)));
        assert_eq!(parse("31"), None);
        assert_eq!(parse("\"0xzz\""), None);
    }
}
