//! Offline audits of a build's linked images, run under the pinned binutils.
//!
//! Each reads `objdump`/`nm` output of the linked ELFs (and the packaged
//! OTA image) and fails closed on anything it cannot classify. They prove
//! properties of the linked code, never runtime behavior; each result carries
//! a `scope` saying what it does not establish. The build records them in the
//! manifest and the frozen review re-runs the linked-code ones.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::{Error, PROJECT, Repo, Result, relative, sha256, text};

pub mod hooks;
pub mod layout;
pub mod mains;
pub mod packaged;
pub mod park;
pub mod pinmap;
pub mod stack;
pub mod writes;

/// One `objdump -d` instruction (or `.word`): address, mnemonic, operands.
pub static INSTRUCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([0-9a-f]+):\s+(?:(?:[0-9a-f]{4}|[0-9a-f]{8})\s+)+([a-z.][a-z0-9.]*)\s*(.*)$")
        .unwrap()
});
/// A line that addresses something, which then has to parse as [`INSTRUCTION`].
pub static ADDRESSED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[0-9a-f]+:\s").unwrap());
/// Unconditional and conditional direct branches (not calls).
pub static DIRECT_BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^b(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[nw])?$").unwrap()
});
/// A function heading in a disassembly: `0e000020 <name>:`.
pub static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9a-f]+) <([^>]+)>:$").unwrap());

pub fn refuse<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Refused(message.into()))
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
        .map_err(|_| Error::Refused(format!("Not a hex number: {text}")))
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

/// A core's linked image in an SDK worktree (`lp` is KM0, `hp` KM4).
pub fn elf(sdk: &Path, core: &str) -> PathBuf {
    sdk.join(PROJECT)
        .join(format!("project_{core}/asdk/image/target_img2.axf"))
}

pub fn ota_image(sdk: &Path) -> PathBuf {
    sdk.join(PROJECT).join("project_hp/asdk/image/OTA_All.bin")
}

pub fn file_sha256(path: &Path) -> Result<String> {
    Ok(sha256(&fs::read(path)?))
}

/// The address of KM0's linked `pmap_func`, which the pad audit reads.
pub fn pmap_address(sdk: &Path, binutils: &Binutils) -> Result<u64> {
    let symbols = tool(&binutils.nm(), &["--defined-only"], &elf(sdk, "lp"))?;
    let matches: Vec<&str> = symbols
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.len() == 3 && fields[2] == "pmap_func")
        .map(|fields| fields[0])
        .collect();
    match matches[..] {
        [address] => parse_hex(address),
        _ => refuse("Expected one linked KM0 pmap_func symbol"),
    }
}

/// The audits of a finished first-boot build, keyed as its manifest
/// records them. `objects` are the KM0/KM4 Rust archives it linked.
pub fn linked(repo: &Repo, build: &Path, objects: &[(String, PathBuf)]) -> Result<Value> {
    let sdk = build.join("sdk");
    let binutils = Binutils {
        dir: repo.binutils()?,
    };
    let stock = fs::read(repo.stock())?;
    let image = fs::read(ota_image(&sdk))?;
    let app = fs::read_to_string(repo.path(stack::APP_SOURCE))?;

    Ok(json!({
        "layout_audit": layout::audit(&ota_image(&sdk), &repo.stock(), &repo.layout())?,
        "km0_pinmap_audit": pinmap::audit_linked_table(&image, pmap_address(&sdk, &binutils)?, &stock)?,
        "ram_park_audit": park::audit(&sdk, &binutils, objects)?,
        "sdk_hook_audit": hooks::audit(&sdk, &binutils, objects)?,
        "firstboot_startup_audit": mains::audit(&sdk, &binutils)?,
        "wifi_link_audit": writes::audit(&sdk, &binutils)?,
        "stack_audit": stack::audit(&elf(&sdk, "hp"), &binutils.objdump(), &app)?,
    }))
}

/// An archive's path as the audits record it: relative to the build output.
pub fn object_path(object: &Path, sdk: &Path) -> Result<String> {
    relative(object, sdk.parent().unwrap_or(sdk))
}
