//! The ELF text a linked-code audit reads must be the text that was packaged
//! (from `verify_packaged_code` in `src/tools/audit/audit-sdk-flash-users.py`),
//! so a stale ELF is refused even if older packaged artifacts still match.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use wellfed_formats::audit::PackagedText;
use wellfed_formats::layout::Hex;
use wellfed_image::ota::Image;

use super::{index, parse_hex, tool};
use crate::platform::amebad::Core;
use crate::{Result, sha256};
use wellfed_formats::require;

/// Why a core's linked text differs from what was packaged.
#[derive(Debug, thiserror::Error)]
pub enum PackagedError {
    #[error("No packaged region {region}")]
    NoRegion { region: String },
    #[error("ELF text is outside packaged region: {section}")]
    OutsideRegion { section: String },
    #[error("ELF text does not match packaged image: {section}")]
    Differs { section: String },
    #[error("Expected exactly the packaged XIP and RAM text sections")]
    Sections { found: Vec<String> },
}

static TEXT_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*\d+\s+(\.(?:ram|xip)_image2\.text)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)",
    )
    .expect("section pattern")
});

/// Byte-match a core's RAM and XIP text against the packaged regions of the
/// OTA payload.
pub(crate) fn verify(
    elf: &Path,
    objdump: &Path,
    payload: &[u8],
    regions: &[Image],
    core: Core,
) -> Result<Vec<PackagedText>> {
    let data = fs::read(elf)?;
    let mut checked = Vec::new();

    for line in tool(objdump, &["-h"], elf)?.lines() {
        let Some(m) = TEXT_SECTION.captures(line) else {
            continue;
        };
        let name = &m[1];
        let (size, address, offset) = (
            index(parse_hex(&m[2])?)?,
            parse_hex(&m[3])?,
            index(parse_hex(&m[4])?)?,
        );

        let region_name = format!(
            "{}{}",
            core.name().to_lowercase(),
            if name.starts_with(".ram") {
                "_ram"
            } else {
                "_xip"
            }
        );
        let Some(region) = regions.iter().find(|r| r.name == region_name) else {
            return Err(PackagedError::NoRegion {
                region: region_name,
            }
            .into());
        };
        let runtime = parse_hex(&region.runtime_address)?;

        let relative = address
            .checked_sub(runtime)
            .and_then(|r| usize::try_from(r).ok());
        let Some(relative) = relative
            .filter(|r| size > 0 && offset + size <= data.len() && r + size <= region.body_bytes)
        else {
            return Err(PackagedError::OutsideRegion {
                section: name.into(),
            }
            .into());
        };
        let start = region.payload_header_offset + 32 + relative;
        require(
            payload.get(start..start + size) == Some(&data[offset..offset + size]),
            || PackagedError::Differs {
                section: name.into(),
            },
        )?;

        checked.push(PackagedText {
            section: name.to_string(),
            address: Hex(address),
            bytes: size,
            sha256: sha256(&data[offset..offset + size]),
        });
    }

    let mut sections: Vec<&str> = checked.iter().map(|c| c.section.as_str()).collect();
    sections.sort();
    require(
        checked.len() == 2 && sections == [".ram_image2.text", ".xip_image2.text"],
        || PackagedError::Sections {
            found: sections.iter().map(|s| s.to_string()).collect(),
        },
    )?;
    Ok(checked)
}
