//! The OTA container and placement audit of a candidate image against the
//! verified stock layout (port of `tools/audit/audit-ota-layout.py`).
//!
//! Structural and known-range only: not a flash plan or full partition
//! validation, and boot acceptance stays unproven.

use std::fs;
use std::path::Path;

use wellfed_flash::ota::{Image, STOCK_SHA256, container, images};
use wellfed_formats::layout::{Hex, Layout};
use serde::Serialize;

use super::word;
use crate::{Result, require, sha256};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LayoutAudit {
    schema_version: u32,
    image: String,
    bytes: usize,
    sha256: String,
    payload_bytes: usize,
    /// The core images, as the flash client's container check reads them.
    regions: Vec<Image>,
    candidate_placements: Vec<Placement>,
    checks: LayoutChecks,
    boot_acceptance: &'static str,
    physical_flash_capacity: &'static str,
    scope: &'static str,
}

/// The candidate written at one bank.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Placement {
    bank_index: usize,
    flash_offset: Hex,
    end_exclusive: Hex,
    erase_bytes: usize,
}

/// What the audit refused otherwise; always `true` in a written audit.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct LayoutChecks {
    container_and_checksum: bool,
    reference_table_matches: bool,
    known_preserve_ranges_untouched: bool,
}

pub(crate) fn audit(image: &Path, stock: &Path, layout: &Path) -> Result<LayoutAudit> {
    let layout: Layout = wellfed_formats::load(layout)?;
    require(
        layout.schema_version == 1 && layout.stock_sha256 == STOCK_SHA256,
        "Unsupported layout/reference identity",
    )?;

    let stock = fs::read(stock)?;
    require(
        layout.stock_bytes == stock.len() as u64 && sha256(&stock) == STOCK_SHA256,
        "Stock reference is not the verified PLAF103 1.1.15 image",
    )?;
    let reference = container(&stock)?;

    let offsets: Vec<u64> = layout.ota_bank_offsets.iter().map(|o| o.0).collect();
    require(
        offsets.len() == 2 && offsets[0] < offsets[1],
        "Invalid bank offsets",
    )?;
    let mapped = layout.flash_mapping_base.0;
    let table = (0x17040 + layout.bank_table_runtime_address.0)
        .checked_sub(0x0E00_0020)
        .and_then(|table| usize::try_from(table).ok())
        .unwrap_or(usize::MAX);
    require(
        word(&stock, table) == Some(mapped + offsets[0])
            && word(&stock, table + 4) == Some(mapped + offsets[1]),
        "Layout bank offsets do not match the verified stock table",
    )?;
    require(
        layout.candidate_payload_cap_bytes == reference.len() as u64,
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
        layout.sector_bytes == 4096 && payload.len() % 4096 == 0,
        "Unsupported sector alignment",
    )?;
    let image_regions = images(payload)?;

    let mut placements = Vec::new();
    for (bank_index, start) in offsets.iter().enumerate() {
        let end = start + payload.len() as u64;
        let overlaps: Vec<&str> = layout
            .known_preserve_ranges
            .iter()
            .filter(|range| *start < range.end_exclusive.0 && range.start.0 < end)
            .map(|range| range.name.as_str())
            .collect();
        require(
            overlaps.is_empty(),
            format!("Candidate overlaps known preserved data: {overlaps:?}"),
        )?;
        placements.push(Placement {
            bank_index,
            flash_offset: Hex(*start),
            end_exclusive: Hex(end),
            erase_bytes: payload.len(),
        });
    }

    Ok(LayoutAudit {
        schema_version: 1,
        image: image.display().to_string(),
        bytes: data.len(),
        sha256: sha256(&data),
        payload_bytes: payload.len(),
        regions: image_regions,
        candidate_placements: placements,
        checks: LayoutChecks {
            container_and_checksum: true,
            reference_table_matches: true,
            known_preserve_ranges_untouched: true,
        },
        boot_acceptance: "unproven",
        physical_flash_capacity: "unverified",
        scope: "Offline structural/known-range audit; not a flash plan or full partition validation",
    })
}
