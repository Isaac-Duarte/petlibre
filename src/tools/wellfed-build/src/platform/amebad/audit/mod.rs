//! Offline audits of a build's linked images, run under the pinned binutils.
//!
//! Each reads `objdump`/`nm` output of the linked ELFs (and the packaged
//! OTA image) and fails closed on anything it cannot classify. They prove
//! properties of the linked code, never runtime behavior; each result carries
//! a `scope` saying what it does not establish. The build records them in the
//! manifest and the frozen review re-runs the linked-code ones, comparing
//! them with the manifest's as whole objects, so both come from these types.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use wellfed_formats::audit::{
    ArchiveRecord, HookAudit, LayoutAudit, MainsAudit, ParkAudit, PinmapAudit, StackAudit,
    WritesAudit,
};

use super::{Core, PROJECT, binutils};
use crate::device::StockPaths;
use crate::{Firmware, Repo, Result, relative, text};
use wellfed_formats::require;

pub mod hooks;
pub(crate) mod layout;
pub mod mains;
pub(crate) mod packaged;
pub mod park;
pub(crate) mod pinmap;
pub(crate) mod stack;
pub(crate) mod writes;

/// One `objdump -d` instruction (or `.word`): address, mnemonic, operands.
pub(crate) static INSTRUCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([0-9a-f]+):\s+(?:(?:[0-9a-f]{4}|[0-9a-f]{8})\s+)+([a-z.][a-z0-9.]*)\s*(.*)$")
        .expect("instruction pattern")
});
/// A line that addresses something, which then has to parse as [`INSTRUCTION`].
pub(crate) static ADDRESSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[0-9a-f]+:\s").expect("address pattern"));
/// Unconditional and conditional direct branches (not calls).
pub(crate) static DIRECT_BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^b(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[nw])?$")
        .expect("branch pattern")
});
/// A function heading in a disassembly: `0e000020 <name>:`.
pub(crate) static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9a-f]+) <([^>]+)>:$").expect("heading pattern"));

/// What the audits' shared readers of tool output refuse: each audit fails
/// closed on output it cannot read.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("Not a hex number: {text}")]
    NotHex { text: String },
    #[error("Offset out of range: {value:#x}")]
    Offset { value: u64 },
    #[error("Expected one linked KM0 pmap_func symbol")]
    PmapSymbol { found: usize },
    #[error("No {core} archive")]
    NoArchive { core: Core },
    #[error("KM4 {name} is at {found:?}, not {pinned:#x}")]
    RetainedRam {
        name: &'static str,
        found: Option<u64>,
        pinned: u64,
    },
}

/// Where the KM4 link puts the wall clock's warm-reset record. A record
/// survives an activation only if the next build looks for it at the same
/// address, so every build pins it.
pub(crate) const TIME_CARRY: u64 = 0x1007_bfc0;
/// The call trail's size: `WELLFED_TRAIL_WORDS` words in `example_entry.c`
/// (`wellfed_amebad_sys::TRAIL_WORDS`), which a test checks against the C.
pub(crate) const TRAIL_BYTES: u64 = 220 * 4;
/// Where the KM4 link puts the call trail, just below the time carry, and
/// the heap's end with it.
pub(crate) const STATUS_TRAIL: u64 = TIME_CARRY - TRAIL_BYTES;

/// Refuse a KM4 link whose `wellfed_time_carry`, `wellfed_status_trail` or
/// `__sram_end__` is not where it is pinned.
pub(crate) fn retained_ram(sdk: &Path, binutils: &Binutils) -> Result<()> {
    let table = tool(&binutils.objdump(), &["-t"], &elf(sdk, Core::Hp))?;
    retained_ram_in(&table)
}

/// The check on an `objdump -t` listing.
pub(crate) fn retained_ram_in(table: &str) -> Result<()> {
    for (name, pinned) in [
        ("wellfed_time_carry", TIME_CARRY),
        ("wellfed_status_trail", STATUS_TRAIL),
        ("__sram_end__", STATUS_TRAIL),
    ] {
        let found: Vec<u64> = table
            .lines()
            .filter(|line| line.split_whitespace().last() == Some(name))
            .filter_map(|line| line.split_whitespace().next())
            .map(parse_hex)
            .collect::<Result<_>>()?;
        require(found == [pinned], || AuditError::RetainedRam {
            name,
            found: found.first().copied(),
            pinned,
        })?;
    }
    Ok(())
}

/// Run a binutils tool and return its stdout.
pub fn tool(path: &Path, args: &[&str], elf: &Path) -> Result<String> {
    text(Command::new(path).args(args).arg(elf))
}

/// `0x…` as Python's `hex()` writes it.
pub fn hex(value: u64) -> String {
    format!("{value:#x}")
}

pub fn parse_hex(text: &str) -> Result<u64> {
    u64::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| AuditError::NotHex { text: text.into() }.into())
}

/// A listed size or file offset as an index into the bytes it describes.
pub(crate) fn index(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_| AuditError::Offset { value }.into())
}

/// The little-endian word at `at`, if `data` holds one there.
pub(crate) fn word(data: &[u8], at: usize) -> Option<u64> {
    let bytes = data.get(at..)?.first_chunk::<4>()?;
    Some(u64::from(u32::from_le_bytes(*bytes)))
}

/// The pinned binutils a build's audits use.
#[derive(Debug, Clone)]
pub struct Binutils {
    pub dir: PathBuf,
}

impl Binutils {
    pub fn objdump(&self) -> PathBuf {
        self.dir.join("arm-none-eabi-objdump")
    }

    pub fn nm(&self) -> PathBuf {
        self.dir.join("arm-none-eabi-nm")
    }
}

/// A core's linked image in an SDK worktree.
pub fn elf(sdk: &Path, core: Core) -> PathBuf {
    sdk.join(PROJECT)
        .join(format!("project_{}/asdk/image/target_img2.axf", core.dir()))
}

pub(crate) fn ota_image(sdk: &Path) -> PathBuf {
    sdk.join(PROJECT).join("project_hp/asdk/image/OTA_All.bin")
}

/// The address of KM0's linked `pmap_func`, which the pad audit reads.
pub(crate) fn pmap_address(sdk: &Path, binutils: &Binutils) -> Result<u64> {
    let symbols = tool(&binutils.nm(), &["--defined-only"], &elf(sdk, Core::Lp))?;
    let matches: Vec<&str> = symbols
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.len() == 3 && fields[2] == "pmap_func")
        .map(|fields| fields[0])
        .collect();
    match matches[..] {
        [address] => parse_hex(address),
        _ => Err(AuditError::PmapSymbol {
            found: matches.len(),
        }
        .into()),
    }
}

/// The linked Rust archive of each core, as the manifest records them.
pub type Archives = [(Core, PathBuf)];

/// The audits of a finished first-boot build, keyed as its manifest
/// records them.
#[derive(Debug, Clone, Serialize)]
pub struct Linked {
    pub(crate) layout_audit: LayoutAudit,
    pub(crate) km0_pinmap_audit: PinmapAudit,
    pub(crate) ram_park_audit: ParkAudit,
    pub(crate) sdk_hook_audit: HookAudit,
    pub(crate) firstboot_startup_audit: MainsAudit,
    pub(crate) wifi_link_audit: WritesAudit,
    pub(crate) stack_audit: StackAudit,
}

/// Run every linked-image audit of a first-boot build against `objects`,
/// the KM0/KM4 Rust archives it linked.
pub(crate) fn linked(
    repo: &Repo,
    stock: &StockPaths,
    build: &Path,
    objects: &Archives,
    firmware: &Firmware,
) -> Result<Linked> {
    let sdk = build.join("sdk");
    let binutils = Binutils {
        dir: binutils(repo)?,
    };
    let stock_image = fs::read(&stock.image)?;
    let image = fs::read(ota_image(&sdk))?;
    let app = stack::task_sources(repo, firmware)?;

    Ok(Linked {
        layout_audit: layout::audit(&ota_image(&sdk), &stock.image, &stock.layout)?,
        km0_pinmap_audit: pinmap::audit_linked_table(
            &image,
            pmap_address(&sdk, &binutils)?,
            &stock_image,
        )?,
        ram_park_audit: park::audit(&sdk, &binutils, objects)?,
        sdk_hook_audit: hooks::audit(&sdk, &binutils, objects)?,
        firstboot_startup_audit: mains::audit(&sdk, &binutils)?,
        wifi_link_audit: writes::audit(&sdk, &binutils)?,
        stack_audit: stack::audit(&elf(&sdk, Core::Hp), &binutils.objdump(), &app)?,
    })
}

/// A core's archive record, as the hook and park audits keep it.
pub(crate) fn archive_record(
    core: Core,
    elf: &Path,
    objects: &Archives,
    sdk: &Path,
) -> Result<ArchiveRecord> {
    let Some((_, object)) = objects.iter().find(|(c, _)| *c == core) else {
        return Err(AuditError::NoArchive { core }.into());
    };

    Ok(ArchiveRecord {
        elf_sha256: crate::digest(elf)?,
        object_path: relative(object, sdk.parent().unwrap_or(sdk))?,
        object_sha256: crate::digest(object)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trail_size_is_the_shims() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../firmware/vendor/ports/amebad/example_entry.c"
        );
        let source = fs::read_to_string(path).unwrap();
        let words: u64 = source
            .lines()
            .find_map(|line| line.strip_prefix("#define WELLFED_TRAIL_WORDS "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(words * 4, TRAIL_BYTES);
        assert_eq!(STATUS_TRAIL, 0x1007_bc50);
        assert_eq!(STATUS_TRAIL % 16, 0, "the heap's end stays aligned");
    }

    #[test]
    fn the_time_carry_and_the_trail_are_pinned_with_the_heap_end() {
        let table = "1007bfc0 g       *ABS*\t00000000 wellfed_time_carry\n\
                     1007bc50 g       *ABS*\t00000000 wellfed_status_trail\n\
                     1007bc50 g       .bluetooth_trace.text\t00000000 __sram_end__\n";
        retained_ram_in(table).unwrap();

        let moved = table.replace("1007bc50 g       .b", "1007bfc0 g       .b");
        let missing = table.lines().take(2).collect::<Vec<_>>().join("\n");
        let no_trail = table.replace("1007bc50 g       *ABS*", "1007bfc0 g       *ABS*");
        for (table, name) in [
            (moved, "__sram_end__"),
            (missing, "__sram_end__"),
            (no_trail, "wellfed_status_trail"),
            (table.repeat(2), "wellfed_time_carry"),
        ] {
            match retained_ram_in(&table) {
                Err(crate::Error::Audit(AuditError::RetainedRam { name: found, .. })) => {
                    assert_eq!(found, name);
                }
                other => panic!("{other:?}"),
            }
        }
    }
}
