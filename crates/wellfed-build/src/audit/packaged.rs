//! The ELF text a linked-code audit reads must be the text that was packaged
//! (from `verify_packaged_code` in `tools/audit/audit-sdk-flash-users.py`),
//! so a stale ELF is refused even if older packaged artifacts still match.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::{parse_hex, refuse, tool};
use crate::{Result, require, sha256};

static TEXT_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*\d+\s+(\.(?:ram|xip)_image2\.text)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)",
    )
    .unwrap()
});

/// Byte-match a core's (`KM0`/`KM4`) RAM and XIP text against the packaged
/// regions of the OTA payload.
pub fn verify(
    elf: &Path,
    objdump: &Path,
    payload: &[u8],
    regions: &[Value],
    core: &str,
) -> Result<Vec<Value>> {
    let data = fs::read(elf)?;
    let mut checked = Vec::new();

    for line in tool(objdump, &["-h"], elf)?.lines() {
        let Some(m) = TEXT_SECTION.captures(line) else {
            continue;
        };
        let name = &m[1];
        let (size, address, offset) = (
            parse_hex(&m[2])? as usize,
            parse_hex(&m[3])?,
            parse_hex(&m[4])? as usize,
        );

        let region_name = format!(
            "{}{}",
            core.to_lowercase(),
            if name.starts_with(".ram") {
                "_ram"
            } else {
                "_xip"
            }
        );
        let Some(region) = regions.iter().find(|r| r["name"] == region_name.as_str()) else {
            return refuse(format!("No packaged region {region_name}"));
        };
        let runtime = parse_hex(region["runtime_address"].as_str().unwrap_or_default())?;
        let header = region["payload_header_offset"].as_u64().unwrap_or_default() as usize;
        let body = region["body_bytes"].as_u64().unwrap_or_default() as usize;

        let relative = address.checked_sub(runtime).map(|r| r as usize);
        let Some(relative) =
            relative.filter(|r| size > 0 && offset + size <= data.len() && r + size <= body)
        else {
            return refuse(format!("ELF text is outside packaged region: {name}"));
        };
        let start = header + 32 + relative;
        require(
            payload.get(start..start + size) == Some(&data[offset..offset + size]),
            format!("ELF text does not match packaged image: {name}"),
        )?;

        checked.push(
            json!({"section": name, "address": format!("0x{address:08x}"),
                            "bytes": size, "sha256": sha256(&data[offset..offset + size])}),
        );
    }

    let mut sections: Vec<&str> = checked
        .iter()
        .filter_map(|c| c["section"].as_str())
        .collect();
    sections.sort();
    require(
        checked.len() == 2 && sections == [".ram_image2.text", ".xip_image2.text"],
        "Expected exactly the packaged XIP and RAM text sections",
    )?;
    Ok(checked)
}
