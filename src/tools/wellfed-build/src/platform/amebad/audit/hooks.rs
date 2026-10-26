//! The SDK flash-hook overlay and its routing audit in both ELFs.
//!
//! The pinned SDK's public flash-lock entries are renamed to legacy names so
//! the RAM interposer can define the public ones. This proves the selected
//! linked hook routes, not the behavior of an installed protocol provider or
//! complete mediation of raw ROM/indirect flash users.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use wellfed_formats::audit::{HookAudit, HookImage, Route, Symbol};
use regex::Regex;

use super::{
    Archives, Binutils, DIRECT_BRANCH, HEADING, INSTRUCTION, archive_record, elf, hex, index,
    parse_hex, tool, word,
};
use crate::Result;
use crate::platform::amebad::Core;
use wellfed_formats::require;

/// Why the flash-hook overlay or its routing audit refused.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("Expected exactly one pinned SDK definition: {name}")]
    Definition { name: &'static str },
    #[error("Ambiguous SDK hook symbol: {name}")]
    Ambiguous { name: String },
    #[error("Missing SDK hook symbols: {}", names.join(", "))]
    Missing { names: Vec<&'static str> },
    #[error("Hook address is not in initialized ELF bytes")]
    Uninitialized { address: u64 },
    #[error("IPC table is too short")]
    ShortTable { bytes: u64 },
    #[error("Flash IPC table bypasses the public hook")]
    IpcBypass,
    #[error("SDK caller bypasses hook: {caller} -> {legacy}")]
    Bypass {
        caller: String,
        legacy: &'static str,
    },
    #[error("Missing visible wrapper route to legacy hook")]
    NoRoute,
    #[error("SDK hook function is outside application RAM: {name}")]
    FunctionPlacement { name: &'static str },
    #[error("SDK hook state is outside internal application RAM: {name}")]
    StatePlacement { name: &'static str },
}

pub(crate) const SOURCE: &str =
    "component/soc/realtek/amebad/fwlib/ram_common/rtl8721d_flash_ram.c";
pub const RENAMES: [(&str, &str); 3] = [
    ("FLASH_Write_Lock", "wellfed_sdk_flash_legacy_lock"),
    ("FLASH_Write_Unlock", "wellfed_sdk_flash_legacy_unlock"),
    ("FLASH_Write_IPC_Int", "wellfed_sdk_flash_legacy_ipc"),
];
const FUNCTIONS: [&str; 3] = [
    "wellfed_amebad_flash_hooks_install",
    "wellfed_amebad_flash_hooks_armed",
    "hook_fail",
];
const STATE: [&str; 7] = [
    "hook_transport",
    "hook_ticket",
    "hook_last_id",
    "hook_armed",
    "hook_faulted",
    "hook_legacy_locked",
    "hook_lease_held",
];

static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d+\s+\.\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)")
        .expect("section pattern")
});
static BRANCH_TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9a-f]+\s+<([^>]+)>").expect("branch target pattern"));

fn functions() -> BTreeSet<&'static str> {
    RENAMES
        .iter()
        .flat_map(|(public, legacy)| [*public, *legacy])
        .chain(FUNCTIONS)
        .collect()
}

/// Rename the three public definitions; each must occur exactly once.
pub(crate) fn overlay(source: &str) -> Result<String> {
    let mut source = source.to_string();
    for (public, legacy) in RENAMES {
        let pattern = Regex::new(&format!(r"(?m)^(void\s+){}(\s*\()", regex::escape(public)))?;
        require(pattern.find_iter(&source).count() == 1, || {
            HookError::Definition { name: public }
        })?;
        source = pattern
            .replace(&source, |c: &regex::Captures| {
                format!("{}{legacy}{}", &c[1], &c[2])
            })
            .into_owned();
    }
    Ok(source)
}

pub fn symbols(elf: &Path, objdump: &Path) -> Result<BTreeMap<String, Symbol>> {
    let mut wanted: BTreeSet<&str> = functions();
    wanted.extend(STATE);
    wanted.insert("ipc_init_config");

    let mut selected = BTreeMap::new();
    for line in tool(objdump, &["-t"], elf)?.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != 6 || !wanted.contains(fields[5]) {
            continue;
        }
        let name = fields[5].to_string();
        require(!selected.contains_key(&name), || HookError::Ambiguous {
            name: name.clone(),
        })?;
        selected.insert(
            name,
            Symbol {
                address: parse_hex(fields[0])?,
                bytes: parse_hex(fields[4])?,
                kind: fields[2].into(),
                section: fields[3].into(),
            },
        );
    }

    let missing: Vec<&'static str> = wanted
        .iter()
        .filter(|name| !selected.contains_key(**name))
        .copied()
        .collect();
    require(missing.is_empty(), || HookError::Missing { names: missing })?;
    Ok(selected)
}

/// The file offset of `size` initialized bytes at `address` in `elf`.
pub fn file_offset(elf: &Path, objdump: &Path, address: u64, size: u64) -> Result<usize> {
    let length_of_file = fs::metadata(elf)?.len();
    for line in tool(objdump, &["-h"], elf)?.lines() {
        let Some(m) = SECTION.captures(line) else {
            continue;
        };
        let (length, start, offset) = (parse_hex(&m[1])?, parse_hex(&m[2])?, parse_hex(&m[3])?);
        if start <= address && address + size <= start + length {
            let at = offset + address - start;
            if at + size <= length_of_file {
                return index(at);
            }
        }
    }
    Err(HookError::Uninitialized { address }.into())
}

/// The pinned IPC_INIT_TABLE has three 32-bit members per entry; channel
/// 2's handler is at 2*12+4 and must be the public Thumb entry, not legacy.
pub fn verify_ipc_table(
    elf: &Path,
    objdump: &Path,
    names: &BTreeMap<String, Symbol>,
) -> Result<u64> {
    let table = &names["ipc_init_config"];
    require(table.bytes >= 36, || HookError::ShortTable {
        bytes: table.bytes,
    })?;
    let at = file_offset(elf, objdump, table.address + 28, 4)?;
    let public = names["FLASH_Write_IPC_Int"].address | 1;
    match word(&fs::read(elf)?, at) {
        Some(pointer) if pointer == public => Ok(pointer),
        _ => Err(HookError::IpcBypass.into()),
    }
}

/// Each legacy entry's routes.
pub type Routes = BTreeMap<&'static str, Vec<Route>>;

/// Every direct branch to a legacy entry, which must come from its public
/// wrapper.
pub fn legacy_routes(disassembly: &str) -> Result<Routes> {
    let expected: BTreeMap<&'static str, &'static str> =
        RENAMES.iter().map(|(p, l)| (*l, *p)).collect();
    let mut routes: Routes = expected.keys().map(|l| (*l, Vec::new())).collect();
    let mut owner: Option<String> = None;

    for line in disassembly.lines() {
        if let Some(heading) = HEADING.captures(line) {
            owner = Some(heading[2].to_string());
            continue;
        }
        let Some(m) = INSTRUCTION.captures(line) else {
            continue;
        };
        if !(DIRECT_BRANCH.is_match(&m[2]) || &m[2] == "bl") {
            continue;
        }
        let Some(target) = BRANCH_TARGET.captures(&m[3]) else {
            continue;
        };
        let Some((legacy, public)) = expected.get_key_value(&target[1]) else {
            continue;
        };

        let caller = owner.as_deref().unwrap_or("None");
        require(caller == *public, || HookError::Bypass {
            caller: caller.into(),
            legacy,
        })?;
        if let Some(calls) = routes.get_mut(legacy) {
            calls.push(Route {
                caller: caller.to_string(),
                address: format!("0x{}", &m[1]),
            });
        }
    }

    require(routes.values().all(|calls| !calls.is_empty()), || {
        HookError::NoRoute
    })?;
    Ok(routes)
}

/// `objects` maps KM0/KM4 to the linked Rust archive containing this code.
pub(crate) fn audit(sdk: &Path, binutils: &Binutils, objects: &Archives) -> Result<HookAudit> {
    let objdump = binutils.objdump();
    let mut images = BTreeMap::new();

    for core in Core::BOTH {
        let elf = elf(sdk, core);
        let names = symbols(&elf, &objdump)?;
        for name in functions() {
            let symbol = &names[name];
            require(
                symbol.kind == "F" && symbol.section == ".ram_image2.text",
                || HookError::FunctionPlacement { name },
            )?;
        }
        for name in STATE {
            let symbol = &names[name];
            require(
                symbol.kind == "O" && symbol.section == ".ram_image2.bss",
                || HookError::StatePlacement { name },
            )?;
        }

        let pointer = verify_ipc_table(&elf, &objdump, &names)?;
        let listing = tool(
            &objdump,
            &["-d", "-j", ".ram_image2.text", "-j", ".xip_image2.text"],
            &elf,
        )?;
        let routes = legacy_routes(&listing)?;

        let archive = archive_record(core, &elf, objects, sdk)?;
        images.insert(
            core.name().to_string(),
            HookImage {
                archive,
                symbols: names,
                legacy_entry_routes: routes
                    .into_iter()
                    .map(|(legacy, routes)| (legacy.to_string(), routes))
                    .collect(),
                ipc_channel_2_handler: hex(pointer),
            },
        );
    }

    Ok(HookAudit {
        images,
        protocol_installed: false,
        global_exclusion_proven: false,
        scope: "Public SDK hooks and IPC table route through RAM interposer; \
                legacy entry branches originate only in that interposer. \
                Indirect callers, provider implementation and global exclusion are unproven."
            .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    const PINNED: &str = "void FLASH_Write_Lock(void)\n{\n}\nvoid FLASH_Write_Unlock(void)\n{\n}\n\
                          void FLASH_Write_IPC_Int(VOID *Data, u32 IrqStatus, u32 ChanNum)\n{\n\
                          \tFLASH_Write_Lock();\n}\n";

    #[test]
    fn definitions_renamed_and_internal_callers_preserved() {
        let changed = overlay(PINNED).unwrap();
        for (public, legacy) in RENAMES {
            assert_eq!(changed.matches(&format!("void {legacy}(")).count(), 1);
            assert!(!changed.contains(&format!("void {public}(")));
        }
        assert!(changed.contains("\tFLASH_Write_Lock();"));
    }

    #[test]
    fn missing_or_duplicate_sdk_definition_is_rejected() {
        for (public, _) in RENAMES {
            let renamed = PINNED.replace(&format!("void {public}("), "void unrelated_hook(");
            for source in [format!("{PINNED}\nvoid {public}(void) {{}}\n"), renamed] {
                assert!(matches!(
                    overlay(&source),
                    Err(Error::Hooks(HookError::Definition { name })) if name == public
                ));
            }
        }
    }

    #[test]
    fn legacy_entry_bypass_is_rejected() {
        let wrapped = "10000000 <FLASH_Write_Lock>:\n 10000000:\tf000 f800 \tbl\t10000100 <wellfed_sdk_flash_legacy_lock>\n\
                       10000010 <FLASH_Write_Unlock>:\n 10000010:\tf000 f800 \tbl\t10000200 <wellfed_sdk_flash_legacy_unlock>\n\
                       10000020 <FLASH_Write_IPC_Int>:\n 10000020:\tf000 b800 \tb.w\t10000300 <wellfed_sdk_flash_legacy_ipc>\n";
        let routes = legacy_routes(wrapped).unwrap();
        assert_eq!(
            routes["wellfed_sdk_flash_legacy_ipc"][0].caller,
            "FLASH_Write_IPC_Int"
        );

        let bypass = format!(
            "{wrapped}00000000 <unmediated_sdk_caller>:\n 0:\tf000 f800 \tbl\t10000100 <wellfed_sdk_flash_legacy_lock>\n"
        );
        let error = legacy_routes(&bypass).unwrap_err();
        assert!(
            matches!(
                &error,
                Error::Hooks(HookError::Bypass { caller, legacy })
                    if caller == "unmediated_sdk_caller"
                        && *legacy == "wellfed_sdk_flash_legacy_lock"
            ),
            "{error}"
        );
    }
}
