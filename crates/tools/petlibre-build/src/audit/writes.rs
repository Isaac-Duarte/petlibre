//! Flash-write and eFuse-write reachability in both core images (FW07).
//!
//! Linking the Wi-Fi driver brings in code that can program eFuse, which is
//! one-time programmable, and the SDK's Wi-Fi flash users default to sectors
//! inside the application bank. KM0 also links the WLAN firmware
//! (lib_wifi_fw.a). This checks the linked images, not runtime behavior:
//! references are direct branches (through veneers too) and 32-bit words
//! anywhere in loaded bytes equal to a function's address, attributed to the
//! enclosing symbol. Register-computed calls are not visible here.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::{Binutils, HEADING, elf, index, parse_hex, refuse, tool, word};
use crate::{Core, Result, digest, require};

/// Never in the image: eFuse MAC programming, eFuse-command issuers, the SDK
/// Wi-Fi/FTL flash writers, the mbed flash write API and any shell entry.
const ABSENT: [&str; 19] = [
    "rtw_efuse_boot_write",
    "wifi_set_mac_address",
    "wlan_write_reconnect_data_to_flash",
    "restore_wifi_info_to_flash",
    "example_wlan_fast_connect",
    "ftl_init",
    "flash_erase_sector",
    "flash_erase_block",
    "flash_erase_chip",
    "flash_stream_write",
    "flash_write_word",
    "flash_burst_write",
    "flash_write_protect",
    "flash_set_status",
    "shell_init_ram",
    "shell_task_ram",
    "shell_init_rom",
    "shell_task_rom",
    "shell_loop_rom",
];

static WRITES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(EFUSE_LogicalMap_Write|EFUSE_PG_Packet|EFUSEWrite8|EFUSE_PMAP_WRITE8|FLASH_(Erase|EraseXIP|EreaseDwordsXIP|SetStatus|SetStatusBits|SetStatusXIP|SetStatusBitsXIP|SetSpiMode|WriteEn|WriteStream|TxData\w*|TxCmd\w*))$",
    )
    .expect("literal pattern")
});

type Pins = &'static [(&'static str, &'static [&'static str])];

/// Every reference to a KM4 write entry, pinned. The RAM *XIP wrappers call
/// their ROM counterparts; none of the wrappers but FLASH_EreaseDwordsXIP is
/// referenced, and it is not. The driver reaches eFuse only through its
/// private-command table, and nothing linked issues an eFuse command (the
/// issuers are absent). cmd_flash and cmd_efuse_protect live in the vendor
/// shell command table, which the linker script keeps; no shell entry is
/// linked, and the table's only other reference is the empty init arrays
/// that share its address.
const EXPECTED_HP: Pins = &[
    (
        "EFUSE_LogicalMap_Write",
        &["rtw_config_map_write", "cmd_efuse_protect"],
    ),
    ("EFUSEWrite8", &["cmd_efuse_protect"]),
    ("rtw_config_map_write", &["rtw_ex_set"]),
    ("rtw_ex_set", &["rtw_private_handler"]),
    ("FLASH_Erase", &["FLASH_EraseXIP", "cmd_flash"]),
    ("FLASH_SetStatus", &["FLASH_SetStatusXIP", "cmd_flash"]),
    ("FLASH_SetStatusBits", &["FLASH_SetStatusBitsXIP"]),
    (
        "FLASH_TxData12B",
        &["FLASH_TxData12BXIP", "FLASH_WriteStream", "cmd_flash"],
    ),
    ("FLASH_TxData256B", &["FLASH_TxData256BXIP"]),
    // FW10: the recovery updater's bank-1 wrappers, which refuse unless the
    // image runs from bank 0 and stay inside bank 1 (amebad-sdk target.rs).
    // FW11: the storage-region wrappers (0x340000-0x350000), which the
    // normal role's confirmation write uses.
    (
        "FLASH_EraseXIP",
        &[
            "FLASH_EreaseDwordsXIP",
            "petlibre_amebad_sys::target::bank1_erase",
            "petlibre_amebad_sys::target::storage_erase",
        ],
    ),
    // FW10 give-way: bank 1's first marker byte only, from bank 1, with bank 0 valid.
    (
        "FLASH_TxData256BXIP",
        &[
            "petlibre_amebad_sys::target::bank1_program",
            "petlibre_amebad_sys::target::storage_program",
            "petlibre_amebad_sys::target::bank1_give_way",
        ],
    ),
    ("FLASH_TxData12BXIP", &["FLASH_EreaseDwordsXIP"]),
    ("FLASH_EreaseDwordsXIP", &[]),
    ("cmd_flash", &["shell_cmd_table"]),
    ("cmd_efuse_protect", &["shell_cmd_table"]),
    // The table's help command walks it and is itself only a table entry.
    ("shell_cmd_table", &["__libc_init_array", "CmdRamHelp"]),
    ("CmdRamHelp", &["shell_cmd_table"]),
];

/// KM0 with the WLAN firmware linked: lib_wifi_fw.a itself imports no flash,
/// eFuse or shell symbol. The SDK's flash setup (flash_operation_config,
/// flash_rx_mode_switch) writes the status register at boot, before KM4
/// runs. The ROM shell is a symbol only; nothing references its entries.
const EXPECTED_LP: Pins = &[
    ("EFUSE_LogicalMap_Write", &["cmd_efuse_protect"]),
    (
        "EFUSEWrite8",
        &["cmd_efuse_extpath_write", "cmd_efuse_protect"],
    ),
    ("EFUSE_PG_Packet", &[]),
    ("FLASH_Erase", &["FLASH_EraseXIP"]),
    (
        "FLASH_SetStatus",
        &[
            "FLASH_SetStatusXIP",
            "flash_operation_config",
            "flash_rx_mode_switch",
        ],
    ),
    ("FLASH_SetStatusBits", &["FLASH_SetStatusBitsXIP"]),
    (
        "FLASH_TxData12B",
        &["FLASH_TxData12BXIP", "FLASH_WriteStream"],
    ),
    // FW11: the boot trial clears bank 1's first marker byte, before KM4 runs.
    (
        "FLASH_TxData256B",
        &["FLASH_TxData256BXIP", "petlibre_trial_program"],
    ),
    ("FLASH_EraseXIP", &["FLASH_EreaseDwordsXIP"]),
    ("FLASH_TxData12BXIP", &["FLASH_EreaseDwordsXIP"]),
    ("FLASH_EreaseDwordsXIP", &[]),
    ("FLASH_WriteStream", &[]),
    ("cmd_flash", &["shell_cmd_table"]),
    ("cmd_efuse_protect", &["shell_cmd_table"]),
    ("cmd_efuse_extpath_write", &["cmd_efuse_protect"]),
    ("shell_cmd_table", &["CmdRamHelp"]),
    ("CmdRamHelp", &["shell_cmd_table"]),
    ("shell_init_rom", &[]),
    ("shell_task_rom", &[]),
];

/// Calls and branches, conditional ones included (`bleq` inside an IT block).
static BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:blx?|b)(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le|al)?(?:\.[wn])?\s+([0-9a-f]+)\s+<([^>+]+)>",
    )
    .expect("literal pattern")
});
/// A disassembled line with its encoding: address, bytes, mnemonic.
static INSTRUCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([0-9a-f]+):\t((?:[0-9a-f]{2,8} ?)+)\s*\t(\S+)").expect("literal pattern")
});
static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d+\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)")
        .expect("literal pattern")
});
static IDENTIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^u?(\d+)_?").expect("literal pattern"));
static DISAMBIGUATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^s[0-9A-Za-z]*_").expect("literal pattern"));

/// A Rust v0 nested path (`_RNvNt...`) as `crate::module::item`, without the
/// per-build crate hash, so pins survive rebuilds; other names unchanged.
pub(crate) fn demangle(name: &str) -> String {
    fn ident(text: &str, at: &mut usize) -> Option<String> {
        let m = IDENTIFIER.captures(text.get(*at..)?)?;
        *at += m.get(0)?.end();
        let len: usize = m[1].parse().ok()?;
        let value = text.get(*at..*at + len)?.to_string();
        *at += len;
        Some(value)
    }

    fn path(text: &str, at: &mut usize) -> Option<Vec<String>> {
        let tag = text.get(*at..*at + 1)?;
        *at += 1;
        match tag {
            "C" => {
                if let Some(m) = DISAMBIGUATOR.find(text.get(*at..)?) {
                    *at += m.end();
                }
                Some(vec![ident(text, at)?])
            }
            "N" => {
                // Namespace: v (value), t (type) or another.
                *at += 1;
                let mut parts = path(text, at)?;
                parts.push(ident(text, at)?);
                Some(parts)
            }
            _ => None,
        }
    }

    let Some(text) = name.strip_prefix("_R") else {
        return name.to_string();
    };
    let mut at = 0;
    match path(text, &mut at) {
        Some(parts) if at == text.len() => parts.join("::"),
        _ => name.to_string(),
    }
}

/// {name: (address, section)}; ROM functions come from the absolute symbol
/// file. The first definition of a name wins.
fn table(elf: &Path, objdump: &Path) -> Result<BTreeMap<String, (u64, String)>> {
    let mut names = BTreeMap::new();
    for line in tool(objdump, &["-t"], elf)?.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5
            || fields[0].len() != 8
            || !fields[0]
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            continue;
        }
        let (name, section) = (fields[fields.len() - 1], fields[fields.len() - 3]);
        if section == "*UND*" || name.starts_with('$') || name.starts_with('.') {
            continue;
        }
        if !names.contains_key(name) {
            names.insert(
                name.to_string(),
                (parse_hex(fields[0])?, section.to_string()),
            );
        }
    }
    Ok(names)
}

/// (address, bytes) of every section with file contents that loads.
fn loaded(elf: &Path, objdump: &Path) -> Result<Vec<(u64, Vec<u8>)>> {
    let data = fs::read(elf)?;
    let listing = tool(objdump, &["-h"], elf)?;
    let lines: Vec<&str> = listing.lines().collect();

    let mut out = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let Some(m) = SECTION.captures(line) else {
            continue;
        };
        let flags = lines.get(at + 1).copied().unwrap_or_default();
        if flags.contains("LOAD") && flags.contains("CONTENTS") {
            let (size, start, offset) = (
                index(parse_hex(&m[2])?)?,
                parse_hex(&m[3])?,
                index(parse_hex(&m[4])?)?,
            );
            let begin = offset.min(data.len());
            let end = (offset + size).min(data.len());
            out.push((start, data[begin..end].to_vec()));
        }
    }
    Ok(out)
}

/// {target: enclosing symbols that reference it}.
fn references(
    elf: &Path,
    objdump: &Path,
    names: &BTreeMap<String, (u64, String)>,
    targets: &[String],
    sections: &[&str],
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut starts: Vec<(u64, String)> = names
        .iter()
        .filter(|(_, (address, section))| section != "*ABS*" && *address != 0)
        .map(|(name, (address, _))| (address & !1, demangle(name)))
        .collect();
    starts.sort();
    let owner = |location: u64| {
        let at = starts.partition_point(|(address, _)| *address <= location);
        match at.checked_sub(1) {
            Some(at) => starts[at].1.clone(),
            None => format!("{location:#x}"),
        }
    };

    let mut wanted = BTreeMap::new();
    for name in targets {
        let address = names[name].0;
        for value in [address | 1, address & !1] {
            wanted.insert(value, name.clone());
        }
    }
    let mut found: BTreeMap<String, BTreeSet<String>> = targets
        .iter()
        .map(|t| (t.clone(), BTreeSet::new()))
        .collect();

    let mut args = vec!["-d"];
    for section in sections {
        args.extend(["-j", section]);
    }
    let listing = tool(objdump, &args, elf)?;

    // Bytes of decoded instructions. A stored address in code is a
    // literal-pool word, which the disassembler shows as data (.word); an
    // instruction whose encoding happens to equal an address is not a
    // reference (observed: an eor.w in SHA-512 spelled shell_cmd_table's
    // address).
    let mut code = HashSet::new();
    for line in listing.lines() {
        if let Some(m) = INSTRUCTION
            .captures(line)
            .filter(|m| !m[3].starts_with('.'))
        {
            let at = parse_hex(&m[1])?;
            let bytes = m[2].replace(' ', "").len() as u64 / 2;
            code.extend(at..at + bytes);
        }
    }
    for (start, data) in loaded(elf, objdump)? {
        for at in (0..data.len().saturating_sub(3)).step_by(4) {
            let location = start + at as u64;
            if let Some(target) = word(&data, at).and_then(|value| wanted.get(&value))
                && !(0..4).all(|i| code.contains(&(location + i)))
            {
                found
                    .entry(target.clone())
                    .or_default()
                    .insert(owner(location));
            }
        }
    }

    let mut current: Option<String> = None;
    let mut veneers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in listing.lines() {
        if let Some(heading) = HEADING.captures(line) {
            current = Some(demangle(&heading[2]));
            continue;
        }
        let (Some(branch), Some(caller)) = (BRANCH.captures(line), current.as_ref()) else {
            continue;
        };
        let target = &branch[2];
        if let Some(veneered) = target
            .strip_prefix("__")
            .and_then(|t| t.strip_suffix("_veneer"))
        {
            veneers
                .entry(veneered.to_string())
                .or_default()
                .insert(caller.clone());
        } else if target != caller
            && let Some(callers) = found.get_mut(target)
        {
            callers.insert(caller.clone());
        }
    }
    for (target, callers) in veneers {
        if let Some(found) = found.get_mut(&target) {
            found.extend(callers);
        }
    }

    for (target, callers) in found.iter_mut() {
        callers.retain(|n| n != target && !(n.starts_with("__") && n.ends_with("_veneer")));
    }
    Ok(found)
}

/// What one core's image may and must link.
struct Rules {
    absent: Vec<&'static str>,
    expected: Pins,
    sections: &'static [&'static str],
    required: &'static str,
    present: bool,
}

/// One core's linked writers and every reference to them.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CoreWrites {
    elf_sha256: String,
    absent: Vec<&'static str>,
    write_targets: Vec<String>,
    /// Each referenced target's enclosing symbols.
    references: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WritesAudit {
    hp: CoreWrites,
    lp: CoreWrites,
    scope: &'static str,
}

fn audit_core(elf: &Path, objdump: &Path, core: &Rules) -> Result<CoreWrites> {
    let names = table(elf, objdump)?;
    let linked: Vec<&str> = {
        let mut linked: Vec<&str> = core
            .absent
            .iter()
            .filter(|n| names.contains_key(**n))
            .copied()
            .collect();
        linked.sort();
        linked
    };
    require(
        linked.is_empty(),
        format!("Forbidden flash/eFuse writer linked: {}", linked.join(", ")),
    )?;
    require(
        names.contains_key(core.required) == core.present,
        format!(
            "{}{}",
            if core.present {
                "Missing "
            } else {
                "Unexpected "
            },
            core.required
        ),
    )?;

    let expected: BTreeMap<&str, BTreeSet<String>> = core
        .expected
        .iter()
        .map(|(target, callers)| (*target, callers.iter().map(|c| c.to_string()).collect()))
        .collect();
    let targets: Vec<String> = names
        .keys()
        .filter(|name| WRITES.is_match(name) || expected.contains_key(name.as_str()))
        .cloned()
        .collect();
    let found = references(elf, objdump, &names, &targets, core.sections)?;

    let empty = BTreeSet::new();
    let unexpected: BTreeMap<&String, Vec<&String>> = found
        .iter()
        .map(|(target, callers)| {
            let pinned = expected.get(target.as_str()).unwrap_or(&empty);
            (target, callers.difference(pinned).collect::<Vec<_>>())
        })
        .filter(|(_, extra)| !extra.is_empty())
        .collect();
    require(
        unexpected.is_empty(),
        format!("Unreviewed flash/eFuse write reference: {unexpected:?}"),
    )?;
    for (target, callers) in &expected {
        if names.contains_key(*target) && found.get(*target) != Some(callers) {
            return refuse(format!("Pinned flash/eFuse reference changed: {target}"));
        }
    }

    let references = found
        .into_iter()
        .filter(|(_, callers)| !callers.is_empty())
        .collect();
    Ok(CoreWrites {
        elf_sha256: digest(elf)?,
        absent: core.absent.clone(),
        write_targets: targets,
        references,
    })
}

pub(crate) fn audit(sdk: &Path, binutils: &Binutils) -> Result<WritesAudit> {
    let objdump = binutils.objdump();
    let lp_pinned: Vec<&str> = EXPECTED_LP.iter().map(|(t, _)| *t).collect();
    // KM0 needs the WLAN firmware entry for FW07; KM4 must not carry it.
    let hp = Rules {
        absent: ABSENT.to_vec(),
        expected: EXPECTED_HP,
        sections: &[".ram_image2.text", ".xip_image2.text"],
        required: "wifi_FW_init_ram",
        present: false,
    };
    let lp = Rules {
        absent: ABSENT
            .iter()
            .filter(|n| !lp_pinned.contains(n))
            .copied()
            .collect(),
        expected: EXPECTED_LP,
        sections: &[
            ".ram_image2.text",
            ".xip_image2.text",
            ".ram_retention.text",
        ],
        required: "wifi_FW_init_ram",
        present: true,
    };

    Ok(WritesAudit {
        hp: audit_core(&elf(sdk, Core::Hp), &objdump, &hp)?,
        lp: audit_core(&elf(sdk, Core::Lp), &objdump, &lp)?,
        scope: "Linked direct and stored-address references only; register-computed calls, \
                ROM-internal paths (including the WLAN firmware's ROM half) and runtime behavior \
                are not established.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_calls_are_references() {
        for line in [
            " e00cb4c:\tf044 fccc \tbleq\te0514e8 <__FLASH_EraseXIP_veneer>",
            " 1000:\tf000 f800 \tbl\t10005910 <FLASH_EraseXIP>",
            " 1000:\tf000 b800 \tb.w\t10005910 <FLASH_EraseXIP>",
            " 1000:\td900      \tbls.n\t10005910 <FLASH_EraseXIP>",
            " 1000:\t4780      \tblx\t10005910 <FLASH_EraseXIP>",
        ] {
            let m = BRANCH.captures(line).unwrap_or_else(|| panic!("{line}"));
            assert!(["FLASH_EraseXIP", "__FLASH_EraseXIP_veneer"].contains(&&m[2]));
        }
        assert!(BRANCH.captures(" 1000:\t4770 \tbx\tlr").is_none());
    }

    #[test]
    fn instructions_are_code_and_literal_words_are_not() {
        // This eor.w's encoding equals shell_cmd_table's address in one build.
        let insn = INSTRUCTION
            .captures(" e014a28:\tea80 0e06 \teor.w\tlr, r0, r6")
            .unwrap();
        assert_eq!(
            (&insn[1], insn[2].replace(' ', "").as_str(), &insn[3]),
            ("e014a28", "ea800e06", "eor.w")
        );
        let load = INSTRUCTION
            .captures(" e014a2c:\t989e      \tldr\tr0, [sp, #632]")
            .unwrap();
        assert_eq!(&load[3], "ldr");
        let word = INSTRUCTION
            .captures("100057bc:\t100210f8 \t.word\t0x100210f8")
            .unwrap();
        assert!(word[3].starts_with('.'));
    }

    #[test]
    fn rust_paths_drop_the_crate_hash() {
        assert_eq!(
            demangle("_RNvNtCseBw8xjNrJWj_19petlibre_amebad_sys6target13bank1_program"),
            "petlibre_amebad_sys::target::bank1_program"
        );
        assert_eq!(
            demangle("_RNvNtCsOther123_19petlibre_amebad_sys6target11bank1_erase"),
            "petlibre_amebad_sys::target::bank1_erase"
        );
        for name in [
            "FLASH_EraseXIP",
            "cmd_flash",
            "_RNvMs_NtCsx_5crate3modNtB4_4Type3new",
            "_Rgarbage",
        ] {
            assert_eq!(demangle(name), name);
        }
    }
}
