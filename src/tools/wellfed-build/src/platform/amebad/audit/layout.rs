//! The OTA container and placement audit of a candidate image against the
//! verified stock layout (port of `src/tools/audit/audit-ota-layout.py`).
//!
//! Structural and known-range only: not a flash plan or full partition
//! validation, and boot acceptance stays unproven.

use std::fs;
use std::path::Path;

use wellfed_formats::audit::{LayoutAudit, LayoutChecks, LayoutRegion, Placement};
use wellfed_formats::layout::{Hex, Layout};
use wellfed_image::ota::{STOCK_SHA256, container, images};

use super::word;
use crate::{Result, sha256};
use wellfed_formats::require;

/// Why the stock layout, its reference image or a candidate was refused.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("Unsupported layout/reference identity")]
    Identity,
    #[error("Stock reference is not the verified PLAF103 1.1.15 image")]
    Reference,
    #[error("Invalid bank offsets")]
    BankOffsets,
    #[error("Layout bank offsets do not match the verified stock table")]
    BankTable,
    #[error("Unsupported candidate size cap")]
    SizeCap,
    #[error("Candidate exceeds the known stock reference footprint")]
    Footprint,
    #[error("Candidate crosses the next bank start")]
    CrossesBank,
    #[error("Unsupported sector alignment")]
    Alignment,
    #[error("Candidate overlaps known preserved data: {ranges:?}")]
    Preserved { ranges: Vec<String> },
}

pub(crate) fn audit(image: &Path, stock: &Path, layout: &Path) -> Result<LayoutAudit> {
    let layout: Layout = wellfed_formats::load(layout)?;
    require(
        layout.schema_version == 1 && layout.stock_sha256 == STOCK_SHA256,
        || LayoutError::Identity,
    )?;

    let stock = fs::read(stock)?;
    require(
        layout.stock_bytes == stock.len() as u64 && sha256(&stock) == STOCK_SHA256,
        || LayoutError::Reference,
    )?;
    let reference = container(&stock)?;

    let offsets: Vec<u64> = layout.ota_bank_offsets.iter().map(|o| o.0).collect();
    require(offsets.len() == 2 && offsets[0] < offsets[1], || {
        LayoutError::BankOffsets
    })?;
    let mapped = layout.flash_mapping_base.0;
    let table = (0x17040 + layout.bank_table_runtime_address.0)
        .checked_sub(0x0E00_0020)
        .and_then(|table| usize::try_from(table).ok())
        .unwrap_or(usize::MAX);
    require(
        word(&stock, table) == Some(mapped + offsets[0])
            && word(&stock, table + 4) == Some(mapped + offsets[1]),
        || LayoutError::BankTable,
    )?;
    require(
        layout.candidate_payload_cap_bytes == reference.len() as u64,
        || LayoutError::SizeCap,
    )?;

    require(fs::metadata(image)?.len() <= stock.len() as u64, || {
        LayoutError::Footprint
    })?;
    let data = fs::read(image)?;
    let payload = container(&data)?;
    require(payload.len() <= reference.len(), || LayoutError::Footprint)?;
    require(payload.len() as u64 <= offsets[1] - offsets[0], || {
        LayoutError::CrossesBank
    })?;
    require(
        layout.sector_bytes == 4096 && payload.len() % 4096 == 0,
        || LayoutError::Alignment,
    )?;
    let image_regions = images(payload)?;

    let mut placements = Vec::new();
    for (bank_index, start) in offsets.iter().enumerate() {
        let end = start + payload.len() as u64;
        let ranges: Vec<String> = layout
            .known_preserve_ranges
            .iter()
            .filter(|range| *start < range.end_exclusive.0 && range.start.0 < end)
            .map(|range| range.name.clone())
            .collect();
        require(ranges.is_empty(), || LayoutError::Preserved { ranges })?;
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
        regions: image_regions
            .into_iter()
            .map(|region| LayoutRegion {
                name: region.name.into(),
                payload_header_offset: region.payload_header_offset,
                body_bytes: region.body_bytes,
                runtime_address: region.runtime_address,
            })
            .collect(),
        candidate_placements: placements,
        checks: LayoutChecks {
            container_and_checksum: true,
            reference_table_matches: true,
            known_preserve_ranges_untouched: true,
        },
        boot_acceptance: "unproven".into(),
        physical_flash_capacity: "unverified".into(),
        scope:
            "Offline structural/known-range audit; not a flash plan or full partition validation"
                .into(),
    })
}
