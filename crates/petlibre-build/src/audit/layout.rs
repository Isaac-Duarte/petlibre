//! The OTA container and placement audit of a candidate image against the
//! verified stock layout (port of `tools/audit/audit-ota-layout.py`).
//!
//! Structural and known-range only: not a flash plan or full partition
//! validation, and boot acceptance stays unproven.

use std::fs;
use std::path::Path;

use petlibre_flash::ota::{STOCK_SHA256, container, regions};
use serde_json::{Value, json};

use super::parse_hex;
use crate::{Result, require, sha256};

fn word(data: &[u8], at: usize) -> Option<u64> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as u64)
}

fn hex_field(value: &Value) -> Result<u64> {
    parse_hex(value.as_str().unwrap_or_default())
}

pub fn audit(image: &Path, stock: &Path, layout: &Path) -> Result<Value> {
    let layout: Value = serde_json::from_slice(&fs::read(layout)?)?;
    require(
        layout["schema_version"] == 1 && layout["stock_sha256"] == STOCK_SHA256,
        "Unsupported layout/reference identity",
    )?;

    let stock = fs::read(stock)?;
    require(
        layout["stock_bytes"].as_u64() == Some(stock.len() as u64)
            && sha256(&stock) == STOCK_SHA256,
        "Stock reference is not the verified PLAF103 1.1.15 image",
    )?;
    let reference = container(&stock)?;

    let offsets = layout["ota_bank_offsets"]
        .as_array()
        .map(|offsets| offsets.iter().map(hex_field).collect::<Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    require(
        offsets.len() == 2 && offsets[0] < offsets[1],
        "Invalid bank offsets",
    )?;
    let mapped = hex_field(&layout["flash_mapping_base"])?;
    let table = (0x17040 + hex_field(&layout["bank_table_runtime_address"])?)
        .checked_sub(0x0E00_0020)
        .unwrap_or(u64::MAX) as usize;
    require(
        word(&stock, table) == Some(mapped + offsets[0])
            && word(&stock, table + 4) == Some(mapped + offsets[1]),
        "Layout bank offsets do not match the verified stock table",
    )?;
    require(
        layout["candidate_payload_cap_bytes"].as_u64() == Some(reference.len() as u64),
        "Unsupported candidate size cap",
    )?;

    require(
        fs::metadata(image)?.len() <= stock.len() as u64,
        "Candidate exceeds the known stock reference footprint",
    )?;
    let data = fs::read(image)?;
    let payload = container(&data)?;
    require(
        payload.len() <= reference.len(),
        "Candidate exceeds the known stock reference footprint",
    )?;
    require(
        payload.len() as u64 <= offsets[1] - offsets[0],
        "Candidate crosses the next bank start",
    )?;
    require(
        layout["sector_bytes"] == 4096 && payload.len() % 4096 == 0,
        "Unsupported sector alignment",
    )?;
    let image_regions = regions(payload)?;

    let preserved = layout["known_preserve_ranges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut placements = Vec::new();
    for (index, start) in offsets.iter().enumerate() {
        let end = start + payload.len() as u64;
        let mut overlaps = Vec::new();
        for range in &preserved {
            if *start < hex_field(&range["end_exclusive"])? && hex_field(&range["start"])? < end {
                overlaps.push(range["name"].clone());
            }
        }
        require(
            overlaps.is_empty(),
            format!("Candidate overlaps known preserved data: {overlaps:?}"),
        )?;
        placements.push(
            json!({"bank_index": index, "flash_offset": format!("0x{start:08x}"),
                               "end_exclusive": format!("0x{end:08x}"),
                               "erase_bytes": payload.len()}),
        );
    }

    Ok(json!({
        "schema_version": 1, "image": image.display().to_string(), "bytes": data.len(),
        "sha256": sha256(&data), "payload_bytes": payload.len(),
        "regions": image_regions, "candidate_placements": placements,
        "checks": {"container_and_checksum": true, "reference_table_matches": true,
                   "known_preserve_ranges_untouched": true},
        "boot_acceptance": "unproven", "physical_flash_capacity": "unverified",
        "scope": "Offline structural/known-range audit; not a flash plan or full partition validation",
    }))
}
