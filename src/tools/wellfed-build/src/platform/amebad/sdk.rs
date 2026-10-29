//! A profile's private SDK worktree: refused when it drifted, then overlaid
//! from the pinned revision.
//!
//! Every overlay starts from `git show HEAD:<path>` rather than the file on
//! disk, so a rerun never stacks edits, and each one anchors on text that
//! occurs exactly once in the pinned SDK, so a changed SDK is refused rather
//! than silently patched elsewhere. The overlaid files' digests go into the
//! manifest and are checked again after compilation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use regex::Regex;
use wellfed_formats::layout::Layout;
use wellfed_formats::lock::SdkLock;
use wellfed_formats::require;

use super::audit::{TRAIL_BYTES, hooks, pinmap};

use super::{Amebad, Core, PROJECT, credentials_header, source, toolchains};
use crate::device::StockPaths;
use crate::platform::Job;
use crate::{Profile, Provisioning, Repo, Result, Stage, digest, git, revision, run};

/// Why a profile's SDK worktree was refused or could not be overlaid. A
/// refused worktree is left unchanged for review.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// Changes outside the profile's overrides, as `git status` codes and
    /// paths, sorted.
    #[error(
        "SDK worktree has changes outside this profile's overrides; left unchanged. \
         Review, then restore or recreate {}: {}",
        sdk.display(),
        shown(changes)
    )]
    Worktree { sdk: PathBuf, changes: Vec<String> },
    #[error("Pinned SDK file is not UTF-8: {path}")]
    NotUtf8 { path: String },
    /// An overlay's anchor no longer occurs exactly once in the pinned SDK.
    #[error("{0}")]
    Anchor(SdkAnchor),
    /// A staged copy differs from the input the identity hashed.
    #[error("{file}: {name}")]
    Staged { file: StagedFile, name: String },
    #[error("Build worktree has an unexpected SDK revision")]
    Revision,
    #[error("Unexpected toolchain path; left unchanged: {}", path.display())]
    ToolchainPath { path: PathBuf },
}

/// The pinned SDK text each overlay anchors on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SdkAnchor {
    #[error("Pinned hard fault handler changed")]
    FaultHandler,
    #[error("Pinned tick hook option changed")]
    TickHook,
    #[error("Pinned task name length changed")]
    TaskNameLength,
    #[error("Pinned image2 link rule changed")]
    Image2LinkRule,
    #[error("Pinned RAM linker section changed")]
    RamSection,
    #[error("Pinned KM4 RAM end changed")]
    SramEnd,
    #[error("Pinned SDK OTA address definition changed")]
    OtaAddress,
    #[error("Pinned example source list changed")]
    ExampleSources,
    #[error("Pinned BT option changed")]
    BtOption,
    #[error("Pinned FTL source list changed")]
    FtlSources,
    #[error("Pinned BT system data switch read changed")]
    BtSystemData,
    #[error("Pinned fast-connect option changed")]
    FastConnect,
    #[error("Pinned flash speed option changed")]
    FlashSpeed,
}

/// The files staged into the worktree from the checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StagedFile {
    #[error("Source changed while staging")]
    Source,
    #[error("Wi-Fi credentials changed while staging")]
    WifiCredentials,
    #[error("First-boot main changed while staging")]
    FirstbootMain,
}

const PLATFORM_OPTS: &str = "project/realtek_amebaD_va0_example/inc/inc_hp/platform_opts.h";
const WIFI_HEADER: &str = "component/common/example/wellfed_wifi_credentials.h";
/// The broker credentials' header, which builders from before the
/// replacement's MQTT transport was deleted wrote into the SDK worktree.
/// Nothing includes it any more, and it holds the broker password in
/// cleartext, so a worktree that still has it loses it before the check.
const RETIRED_TRANSPORT_HEADER: &str = "component/common/example/wellfed_transport_credentials.h";
const OTA_HEADER: &str = "component/soc/realtek/amebad/fwlib/include/rtl8721d_ota.h";
const PINMAP: &str = "component/soc/realtek/amebad/fwlib/usrcfg/rtl8721dlp_pinmapcfg.c";
const UTILITIES_MAKEFILE: &str = "project_hp/asdk/make/utilities_example/Makefile";
const SRC_LP_MAKEFILE: &str = "project/realtek_amebaD_va0_example/src/src_lp/Makefile";
/// The KM4 build options; `CONFIG_BT_EN` adds the BT board code and `btgap.a`.
const HP_CONFIG: &str = "project_hp/asdk/config/.config";
const HCI_BOARD: &str = "component/common/bluetooth/realtek/sdk/board/amebad/src/hci/hci_board.c";
/// Compiles the SDK's FTL store, whose entry points the BLE peripheral
/// replaces.
const FTL_MAKEFILE: &str = "project_hp/asdk/make/file_system/ftl/Makefile";
/// Where a Bluetooth firmware's BLE peripheral is staged.
const BLE_SOURCE: &str = "component/common/example/wellfed_ble.c";
/// KM4's hard fault handler, which gains a `PLFLT1` line of the fault status
/// registers: its backtrace alone cannot tell an imprecise bus fault
/// from a corrupted pointer.
const APP_START: &str = "component/soc/realtek/amebad/fwlib/ram_hp/rtl8721dhp_app_start.c";
/// KM4's FreeRTOS options, which gain the tick hook the application entry
/// defines (it leaves a breadcrumb that survives a silent watchdog reset)
/// and task names long enough for it to tell the tasks apart.
const FREERTOS_CONFIG: &str = "project/realtek_amebaD_va0_example/inc/inc_hp/FreeRTOSConfig.h";
/// KM0's flash options, whose SPI clock drops from 80 MHz to 50 MHz: at
/// 80 MHz this board's flash returned transient single-bit read errors
/// under load, which XIP turns into faults anywhere in the image.
const FLASH_CONFIG: &str = "component/soc/realtek/amebad/fwlib/usrcfg/rtl8721dlp_flashcfg.c";
/// Object suffixes the vendor makefiles leave beside a source.
const OBJECT_SUFFIXES: [&str; 5] = [".o", ".d", ".i", ".s", ".su"];

fn project(path: &str) -> String {
    format!("{PROJECT}/{path}")
}

/// Exact SDK worktree changes the pinned vendor recipes make themselves,
/// observed in every profile's worktree: `check_execute_permission` sets the
/// executable bit on these helpers (content must stay identical to the
/// pinned revision) ...
fn vendor_mode_only() -> BTreeSet<String> {
    Core::BOTH
        .iter()
        .flat_map(|core| {
            [
                "checksum",
                "image_tool/imagetool.sh",
                "pad.sh",
                "prepend_header.sh",
                "prepend_ota_header.sh",
            ]
            .map(|name| project(&format!("project_{}/asdk/gnu_utility/{name}", core.dir())))
        })
        .collect()
}

/// ... the KM4 recipe re-archives this tracked library on every build ...
fn vendor_rebuilt() -> BTreeSet<String> {
    [project("project_hp/asdk/lib/application/lib_user.a")].into()
}

/// ... and these untracked paths are the wrapper's toolchain links (checked
/// separately) and two KM0 loader outputs copied by the vendor recipe.
fn vendor_untracked() -> BTreeSet<String> {
    [
        "project_hp/toolchain/linux",
        "project_lp/toolchain/linux",
        "project_lp/asdk/gnu_utility/gnu_script/acut_loader/flash_loader_ram_1.bin",
        "project_lp/asdk/gnu_utility/gnu_script/acut_loader/target_FPGA.axf",
    ]
    .map(project)
    .into()
}

/// SDK paths this builder regenerates from the pinned revision for a profile.
fn managed_sdk_paths(profile: Profile) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    if profile.has(Stage::Stock) {
        paths.extend([OTA_HEADER.to_string(), PINMAP.to_string()]);
    }
    if profile.has(Stage::Rust) {
        paths.extend([
            "component/common/example/example_entry.c".to_string(),
            hooks::SOURCE.to_string(),
            project("project_lp/asdk/Makefile"),
            project("project_hp/asdk/Makefile"),
            project("project_lp/asdk/ld/rlx8721d_img2.ld"),
            project("project_hp/asdk/ld/rlx8721d_img2_is.ld"),
            // Restored from the pinned revision before every application build.
            project(UTILITIES_MAKEFILE),
            SRC_LP_MAKEFILE.to_string(),
            // Overlaid for a Bluetooth firmware, restored for any other.
            project(HP_CONFIG),
            project(FTL_MAKEFILE),
            HCI_BOARD.to_string(),
            APP_START.to_string(),
            FREERTOS_CONFIG.to_string(),
            FLASH_CONFIG.to_string(),
        ]);
    }
    if profile.has(Stage::Firstboot) {
        paths.extend(Core::BOTH.map(|core| {
            format!(
                "project/realtek_amebaD_va0_example/src/src_{}/main.c",
                core.dir()
            )
        }));
        paths.insert(PLATFORM_OPTS.to_string());
    }
    paths
}

/// Worktree changes outside the builder's overrides and known vendor
/// outputs, as `<code> <path>`, sorted. Ignored files are governed by the
/// pinned SDK's own ignore rules and are not listed.
fn unexpected_sdk_changes(sdk: &Path, profile: Profile) -> Result<Vec<String>> {
    let status = run(crate::git_command(sdk).args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--no-renames",
    ]))?
    .stdout;
    let managed = managed_sdk_paths(profile);
    let (mode_only, rebuilt, untracked) =
        (vendor_mode_only(), vendor_rebuilt(), vendor_untracked());

    let mut unexpected = Vec::new();
    for entry in String::from_utf8_lossy(&status)
        .split('\0')
        .filter(|e| e.len() > 3)
    {
        let (code, path) = (&entry[..2], &entry[3..]);
        let allowed = match code {
            "??" => {
                untracked.contains(path)
                    || (profile.has(Stage::Rust) && (path == WIFI_HEADER || path == BLE_SOURCE))
            }
            " M" => {
                managed.contains(path)
                    || rebuilt.contains(path)
                    || (mode_only.contains(path)
                        && executable(&sdk.join(path))
                        && git(sdk, &["hash-object", "--", path])?
                            == git(sdk, &["rev-parse", &format!("HEAD:{path}")])?)
            }
            // Staged, deleted, type-changed or conflicted entries.
            _ => false,
        };
        if !allowed {
            let shown = if code.trim().is_empty() {
                code
            } else {
                code.trim()
            };
            unexpected.push(format!("{shown} {path}"));
        }
    }
    unexpected.sort();
    Ok(unexpected)
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

pub(crate) fn check_sdk_worktree(sdk: &Path, profile: Profile) -> Result<()> {
    let unexpected = unexpected_sdk_changes(sdk, profile)?;
    if unexpected.is_empty() {
        return Ok(());
    }

    Err(SdkError::Worktree {
        sdk: sdk.to_path_buf(),
        changes: unexpected,
    }
    .into())
}

/// The first 20 of a worktree's unexpected changes, and how many more.
fn shown(changes: &[String]) -> String {
    let mut shown = changes[..changes.len().min(20)].join(", ");
    if changes.len() > 20 {
        shown.push_str(&format!(" (+{} more)", changes.len() - 20));
    }
    shown
}

/// Where a core's Rust archive is staged beside the SDK worktree.
pub(crate) fn rust_archive(sdk: &Path, core: Core) -> PathBuf {
    sdk.parent()
        .unwrap_or(sdk)
        .join("rust-lib")
        .join(format!("libwellfed_{}.a", core.feature()))
}

/// The pinned revision's copy of an SDK file.
fn pinned(sdk: &Path, relative: &str) -> Result<String> {
    let output = run(crate::git_command(sdk).args(["show", &format!("HEAD:{relative}")]))?;
    String::from_utf8(output.stdout).map_err(|_| {
        SdkError::NotUtf8 {
            path: relative.into(),
        }
        .into()
    })
}

/// Replace the single `anchor` in `text`, refusing as `which` otherwise.
fn replace_once(text: &str, anchor: &str, with: &str, which: SdkAnchor) -> Result<String> {
    require(text.matches(anchor).count() == 1, || {
        SdkError::Anchor(which)
    })?;
    Ok(text.replacen(anchor, with, 1))
}

/// Take the top of KM4's main RAM from the heap for what a warm reset
/// carries to the next boot, where neither the image load, `.bss` clearing
/// nor the heap reaches: the last 64 bytes are `wellfed_time_carry`, the
/// wall clock's record, at the same address in every build so it survives
/// an activation; the [`TRAIL_BYTES`] below them are
/// `wellfed_status_trail`, the call trail a silent hang leaves
/// (`example_entry.c`).
fn retained_ram(linker: &str) -> Result<String> {
    let anchor = "__sram_end__ = ORIGIN(BD_RAM_NS) + LENGTH(BD_RAM_NS);";
    replace_once(
        linker,
        anchor,
        &format!(
            "wellfed_time_carry = ORIGIN(BD_RAM_NS) + LENGTH(BD_RAM_NS) - 64;\n\
             wellfed_status_trail = wellfed_time_carry - {TRAIL_BYTES};\n\
             __sram_end__ = wellfed_status_trail;"
        ),
        SdkAnchor::SramEnd,
    )
}

/// The vendor primitives the KM4 link wraps for the call trail: every call
/// to one from another object, the prebuilt Wi-Fi driver's included, goes
/// through `__wrap_<name>` in `example_entry.c`. Each is an undefined
/// reference in the driver objects that call it. `rtw_enter_critical` and
/// `rtw_exit_critical` stay unwrapped: they guard every driver register
/// access, and a note there stretched the first WPA3 join past the watchdog
/// feeder's 1 s supervision gap.
const TRAIL_WRAPS: [&str; 7] = [
    "save_and_cli",
    "restore_flags",
    "rtw_set_timer",
    "rtw_del_timer",
    "rtw_cancel_timer",
    "wifi_indication",
    "rtw_os_indicate_disconnect",
];

/// The KM4 link rule's additions: the Rust archive, then the trail's wraps.
/// KM0 links only the archive.
fn link_additions(core: Core, archive: &Path) -> String {
    let wraps: String = match core {
        Core::Hp => TRAIL_WRAPS
            .iter()
            .map(|name| format!(" -Wl,--wrap={name}"))
            .collect(),
        Core::Lp => String::new(),
    };
    format!("{}{wraps}", archive.display())
}

/// Print the Cortex-M33 fault status and address registers before the
/// vendor backtrace. CFSR's IMPRECISERR or a BFAR in a peripheral marks a
/// bus fault the stacked PC does not point at. Read-only; the handler still
/// ends in its own loop.
fn fault_registers(pinned: &str) -> Result<String> {
    let anchor = "\tcm_backtrace_fault((uint32_t)(IsPstack ? pstack : mstack), lr_value);";
    replace_once(
        pinned,
        anchor,
        &format!(
            "\tDBG_8195A(\"PLFLT1 cfsr=%x hfsr=%x mmfar=%x bfar=%x lr=%x psp=%d\\r\\n\",\n\
             \t\t*(volatile uint32_t *)0xE000ED28, *(volatile uint32_t *)0xE000ED2C,\n\
             \t\t*(volatile uint32_t *)0xE000ED34, *(volatile uint32_t *)0xE000ED38,\n\
             \t\tlr_value, IsPstack);\n{anchor}"
        ),
        SdkAnchor::FaultHandler,
    )
}

/// Have FreeRTOS call `vApplicationTickHook` from every tick interrupt,
/// and keep 15 characters of a task's name, not 9: at the
/// vendor's 10 every runtime task's name ends at `wellfed-`, and the
/// breadcrumb could not tell them apart. Every KM4 object recompiles with
/// it (the options stamp); of the prebuilt libraries none allocates tasks
/// statically, and the one that reads task names takes only the pointer.
fn freertos_options(pinned: &str) -> Result<String> {
    let anchor = "#define configUSE_TICK_HOOK\t\t\t\t\t\t\t\t0\n";
    let text = replace_once(
        pinned,
        anchor,
        &anchor.replace("\t0\n", "\t1\n"),
        SdkAnchor::TickHook,
    )?;
    let anchor = "#define configMAX_TASK_NAME_LEN\t\t\t\t\t\t\t( 10 )\n";
    replace_once(
        &text,
        anchor,
        &anchor.replace("( 10 )", "( 16 )"),
        SdkAnchor::TaskNameLength,
    )
}

/// Set KM0's flash clock option to 50 MHz, the vendor's slowest calibrated
/// speed. KM0 applies it at every cold boot, before KM4 starts, so both
/// banks read the flash at the clock their own image carries.
fn flash_speed(pinned: &str) -> Result<String> {
    let anchor = "const u16 Flash_Speed = 0xFFFF;";
    replace_once(
        pinned,
        anchor,
        "const u16 Flash_Speed = 0x0FFF;",
        SdkAnchor::FlashSpeed,
    )
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Deletes the retired broker credentials' header, if an older builder left
/// it. Only a regular file is removed; anything else there is left for the
/// worktree check to name.
fn remove_retired_header(sdk: &Path) -> Result<()> {
    let header = sdk.join(RETIRED_TRANSPORT_HEADER);
    if fs::symlink_metadata(&header).is_ok_and(|m| m.file_type().is_file()) {
        remove_file(&header)?;
    }
    Ok(())
}

fn remove_objects(directory: &Path, name: &str) -> Result<()> {
    for suffix in OBJECT_SUFFIXES {
        remove_file(&directory.join(format!("{name}{suffix}")))?;
    }
    Ok(())
}

/// Every file below `directory` (not following links).
fn walk(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// The build's SDK overlays, keyed by SDK-relative path with their digests.
pub(crate) type Overrides = BTreeMap<String, String>;

/// Write an overlay and record its digest.
fn overlay(sdk: &Path, relative: &str, text: &str, overrides: &mut Overrides) -> Result<()> {
    let path = sdk.join(relative);
    write_if_changed(&path, text.as_bytes())?;
    overrides.insert(relative.to_string(), digest(&path)?);
    Ok(())
}

/// Write a staged file only when its bytes differ. The vendor makefiles
/// rebuild every object whose dependency file names a touched file, and
/// nearly all of them include the OTA header and `platform_opts.h`, so
/// rewriting an unchanged overlay recompiled most of the SDK on every build.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() != Some(bytes) {
        fs::write(path, bytes)?;
    }
    Ok(())
}

/// Create or check the profile's worktree and apply its overlays.
/// The job's inputs hold the digests the identity was computed from;
/// staged copies of inputs must still match them.
pub(crate) fn prepare(
    job: &Job,
    board: &Amebad,
    stock: Option<&StockPaths>,
    lock: &SdkLock,
) -> Result<(PathBuf, Overrides)> {
    let (repo, profile) = (job.repo, job.profile);
    let sdk = worktree(repo, &job.output, profile, lock)?;

    let mut overrides = Overrides::new();
    let staged = |source: &Path, copy: &Path, file: StagedFile| -> Result<()> {
        let name = crate::relative(source, &repo.root)?;
        require(job.inputs.get(&name) == Some(&digest(copy)?), || {
            SdkError::Staged { file, name }
        })?;
        Ok(())
    };

    if let Some(stock) = stock {
        ota_addresses(stock, &sdk, &mut overrides)?;
        let source = pinmap::overlay(&pinned(&sdk, PINMAP)?, &fs::read(&stock.image)?)?;
        overlay(&sdk, PINMAP, &source, &mut overrides)?;
    }
    if profile.has(Stage::Rust) {
        rust_overlays(repo, board, job.provisioning, &sdk, &mut overrides, &staged)?;
        let bluetooth = board.bluetooth_source(job.firmware)?;
        bluetooth_overlays(&sdk, bluetooth, &mut overrides, &staged)?;
    }
    if profile.has(Stage::Firstboot) {
        firstboot_overlays(board, &sdk, &mut overrides, &staged)?;
    }
    recompile_on_option_change(&sdk)?;
    Ok((sdk, overrides))
}

/// The profile's SDK worktree at the pinned revision, created on first
/// use and refused when it drifted, with both cores' toolchain links.
fn worktree(repo: &Repo, output: &Path, profile: Profile, lock: &SdkLock) -> Result<PathBuf> {
    let sdk = output.join("sdk");
    if !sdk.exists() {
        fs::create_dir_all(output)?;
        run(crate::git_command(&source(repo))
            .args(["worktree", "add", "--detach"])
            .arg(&sdk)
            .arg(&lock.sdk.revision))?;
    }
    require(revision(&sdk)? == lock.sdk.revision, || SdkError::Revision)?;
    remove_retired_header(&sdk)?;
    check_sdk_worktree(&sdk, profile)?;

    // Both cores use the same verified toolchain, rather than downloading copies.
    let toolchains = toolchains(repo);
    for core in Core::BOTH {
        let link = sdk.join(project(&format!("project_{}/toolchain/linux", core.dir())));
        if link.is_symlink() && fs::canonicalize(&link).ok() == fs::canonicalize(&toolchains).ok() {
            continue;
        }
        require(fs::symlink_metadata(&link).is_err(), || {
            SdkError::ToolchainPath { path: link.clone() }
        })?;
        std::os::unix::fs::symlink(&toolchains, &link)?;
    }
    Ok(sdk)
}

/// The Rust profiles' overlays: the application entry, both cores' link
/// rules and RAM sections, the flash-hook rename and the credential headers.
fn rust_overlays(
    repo: &Repo,
    board: &Amebad,
    provisioning: Provisioning,
    sdk: &Path,
    overrides: &mut Overrides,
    staged: &dyn Fn(&Path, &Path, StagedFile) -> Result<()>,
) -> Result<()> {
    let example = sdk.join("component/common/example");
    // The vendor makefiles name objects by basename and include old .d
    // files. Rebuild only our objects to avoid stale links.
    let object_root = sdk.join(project("project_hp/asdk"));
    for name in ["plaf103_flash_park", "plaf103_flash_hooks"] {
        remove_objects(&object_root.join("make/utilities_example"), name)?;
        remove_file(&object_root.join("build/ram").join(format!("{name}.o")))?;
    }

    let destination = example.join("example_entry.c");
    write_if_changed(&destination, &fs::read(&board.entry)?)?;
    staged(&board.entry, &destination, StagedFile::Source)?;

    // Earlier C builds modified these SDK makefiles; restore the pinned copies.
    for relative in [project(UTILITIES_MAKEFILE), SRC_LP_MAKEFILE.to_string()] {
        git(sdk, &["checkout", "HEAD", "--", &relative])?;
    }

    // Both cores link the Rust archive: the KM4 application plus, on
    // both, the RAM substrate and SDK interposer.
    for core in Core::BOTH {
        let relative = project(&format!("project_{}/asdk/Makefile", core.dir()));
        let anchor = "target_img2.axf $(RAM_OBJS_LIST)";
        let archive = rust_archive(sdk, core);
        let text = replace_once(
            &pinned(sdk, &relative)?,
            anchor,
            &format!("{anchor} {}", link_additions(core, &archive)),
            SdkAnchor::Image2LinkRule,
        )?;
        overlay(sdk, &relative, &text, overrides)?;
    }
    for directory in [
        project("project_lp/asdk/make/project/src"),
        project("project_lp/asdk/build/ram"),
        "project/realtek_amebaD_va0_example/src/src_lp".to_string(),
    ] {
        for name in ["plaf103_flash_park", "plaf103_flash_hooks"] {
            remove_objects(&sdk.join(&directory), name)?;
        }
    }
    for (core, linker) in [
        (Core::Lp, "rlx8721d_img2.ld"),
        (Core::Hp, "rlx8721d_img2_is.ld"),
    ] {
        let relative = project(&format!("project_{}/asdk/ld/{linker}", core.dir()));
        let anchor = "*(.image2.ram.text*)";
        let mut text = replace_once(
            &pinned(sdk, &relative)?,
            anchor,
            &format!("KEEP(*(.image2.ram.text.wellfed_park))\n\t\t{anchor}"),
            SdkAnchor::RamSection,
        )?;
        if core == Core::Hp {
            text = retained_ram(&text)?;
        }
        overlay(sdk, &relative, &text, overrides)?;
    }
    let text = hooks::overlay(&pinned(sdk, hooks::SOURCE)?)?;
    overlay(sdk, hooks::SOURCE, &text, overrides)?;
    let text = fault_registers(&pinned(sdk, APP_START)?)?;
    overlay(sdk, APP_START, &text, overrides)?;
    let text = freertos_options(&pinned(sdk, FREERTOS_CONFIG)?)?;
    overlay(sdk, FREERTOS_CONFIG, &text, overrides)?;
    let text = flash_speed(&pinned(sdk, FLASH_CONFIG)?)?;
    overlay(sdk, FLASH_CONFIG, &text, overrides)?;

    // SDK dependencies can retain the old public definitions otherwise.
    for core in Core::BOTH {
        let mut files = Vec::new();
        walk(
            &sdk.join(project(&format!("project_{}/asdk", core.dir()))),
            &mut files,
        )?;
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let suffix = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()));
            if name.starts_with("rtl8721d_flash_ram.")
                && suffix.is_some_and(|s| OBJECT_SUFFIXES.contains(&s.as_str()))
            {
                fs::remove_file(path)?;
            }
        }
    }

    // The entry always includes the header; a consumer image's names no
    // network.
    let (header, source) = credentials_header(repo, provisioning)?;
    write_if_changed(&sdk.join(WIFI_HEADER), header.as_bytes())?;
    if let Some(wifi) = source {
        staged(&wifi, &wifi, StagedFile::WifiCredentials)?;
    }
    Ok(())
}

/// Point the SDK's two OTA bank addresses at the stock layout's banks.
fn ota_addresses(stock: &StockPaths, sdk: &Path, overrides: &mut Overrides) -> Result<()> {
    let layout: Layout = wellfed_formats::load(&stock.layout)?;

    let mut text = pinned(sdk, OTA_HEADER)?;
    for (index, offset) in layout.ota_bank_offsets.iter().enumerate() {
        let address = layout.flash_mapping_base.0 + offset.0;
        let pattern = Regex::new(&format!(
            r"(#define\s+LS_IMG2_OTA{}_ADDR\s+)0x[0-9A-Fa-f]+",
            index + 1
        ))?;
        require(pattern.find_iter(&text).count() == 1, || {
            SdkError::Anchor(SdkAnchor::OtaAddress)
        })?;
        text = pattern
            .replace(&text, |c: &regex::Captures| {
                format!("{}0x{address:08X}", &c[1])
            })
            .into_owned();
    }
    overlay(sdk, OTA_HEADER, &text, overrides)
}

/// A Bluetooth firmware links the BT board code, the vendor host
/// stack and its BLE peripheral; any other build restores the pinned files
/// and drops the staged peripheral, so the two never mix in one worktree.
///
/// Only the makefile option is set. `CONFIG_BT` stays undefined in C, so the
/// boot-time FTL init (`CONFIG_FTL_ENABLED`), the BT AT commands and the
/// vendor examples stay out.
fn bluetooth_overlays(
    sdk: &Path,
    bluetooth: Option<&Path>,
    overrides: &mut Overrides,
    staged: &dyn Fn(&Path, &Path, StagedFile) -> Result<()>,
) -> Result<()> {
    let objects = sdk.join(project("project_hp/asdk"));
    remove_objects(&objects.join("make/utilities_example"), "wellfed_ble")?;
    remove_file(&objects.join("build/ram/wellfed_ble.o"))?;
    let destination = sdk.join(BLE_SOURCE);
    let Some(source) = bluetooth else {
        for relative in [
            project(HP_CONFIG),
            project(FTL_MAKEFILE),
            HCI_BOARD.to_string(),
        ] {
            git(sdk, &["checkout", "HEAD", "--", &relative])?;
        }
        return remove_file(&destination);
    };

    write_if_changed(&destination, &fs::read(source)?)?;
    staged(source, &destination, StagedFile::Source)?;
    // rust_overlays restored this makefile from the pinned revision.
    let makefile = project(UTILITIES_MAKEFILE);
    let anchor = "CSRC += $(DIR)/example_entry.c\n";
    let text = replace_once(
        &pinned(sdk, &makefile)?,
        anchor,
        &format!("{anchor}CSRC += $(DIR)/wellfed_ble.c\n"),
        SdkAnchor::ExampleSources,
    )?;
    overlay(sdk, &makefile, &text, overrides)?;

    let config = project(HP_CONFIG);
    let text = replace_once(
        &pinned(sdk, &config)?,
        "# CONFIG_BT_EN is not set\n",
        "CONFIG_BT_EN=y\n",
        SdkAnchor::BtOption,
    )?;
    overlay(sdk, &config, &text, overrides)?;

    // The vendor stack stores bonds through FTL, which erases and programs a
    // fixed sector inside OTA bank 0. Every build compiles FTL and drops it
    // at link; here the stack references its entry points, so leave it out
    // and let the peripheral's refusing definitions stand in.
    let makefile = project(FTL_MAKEFILE);
    let text = replace_once(
        &pinned(sdk, &makefile)?,
        "CSRC += $(DIR)/ftl.c\n",
        "# wellfed: the BLE peripheral replaces FTL's entry points.\n",
        SdkAnchor::FtlSources,
    )?;
    overlay(sdk, &makefile, &text, overrides)?;

    // The board code reads switches from the system data sector (flash
    // 0x3028), active low: they choose the controller patch (embedded, or
    // read from a fixed address inside our OTA banks), a trace UART on PA26,
    // the controller's log and whether the host stack starts at all. Pin
    // them to an erased sector's values, as the research unit's are, so the
    // image never depends on a unit's system data and never takes a flash
    // lease for this read.
    let anchor = "\tdevice_mutex_lock(RT_DEV_LOCK_FLASH);\n\tFLASH_Write_Lock();\n\tret = \
                  (HAL_READ32(SPI_FLASH_BASE, FLASH_SYSTEM_DATA_ADDR + 0x28) & x);\n\tFLASH_Write_Unlock();\n\t\
                  device_mutex_unlock(RT_DEV_LOCK_FLASH);\n";
    let text = replace_once(
        &pinned(sdk, HCI_BOARD)?,
        anchor,
        "\t/* wellfed: an erased system data sector's switches. */\n\tret = x;\n",
        SdkAnchor::BtSystemData,
    )?;
    overlay(sdk, HCI_BOARD, &text, overrides)
}

/// The KM4 link's object list, which the vendor recipe writes beside the
/// image.
const HP_OBJECT_LIST: &str = "project_hp/asdk/image/obj_list.txt";

/// The vendor makefiles track no header dependencies, so objects compiled
/// under other KM4 options would be linked as they are. Recompile every KM4
/// object when the options, the FreeRTOS options or the build configuration
/// change: every object under the KM4 project, and every object the last
/// KM4 link listed, since some (littlefs, fatfs, parts of the Bluetooth
/// board code) are built beside their sources in `component/`.
fn recompile_on_option_change(sdk: &Path) -> Result<()> {
    let stamp = sdk.parent().unwrap_or(sdk).join("hp-options.sha256");
    let options = format!(
        "{} {} {}",
        digest(&sdk.join(PLATFORM_OPTS))?,
        digest(&sdk.join(project(HP_CONFIG)))?,
        digest(&sdk.join(FREERTOS_CONFIG))?
    );
    if fs::read_to_string(&stamp).ok().as_deref().map(str::trim) == Some(options.as_str()) {
        return Ok(());
    }

    for path in km4_objects(sdk)? {
        remove_file(&path)?;
    }
    fs::write(&stamp, format!("{options}\n"))?;
    Ok(())
}

/// Every KM4 object file in the worktree: those under the KM4 project, and
/// those the last link's object list names, kept only when they resolve
/// inside the worktree.
fn km4_objects(sdk: &Path) -> Result<Vec<PathBuf>> {
    let is_object = |p: &Path| p.extension().is_some_and(|e| e == "o");
    let mut files = Vec::new();
    walk(&sdk.join(project("project_hp/asdk")), &mut files)?;
    files.retain(|p| is_object(p));

    let root = fs::canonicalize(sdk)?;
    if let Ok(list) = fs::read_to_string(sdk.join(project(HP_OBJECT_LIST))) {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let path = Path::new(line);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                sdk.join(project("project_hp/asdk")).join(path)
            };
            if let Ok(resolved) = fs::canonicalize(&path)
                && resolved.starts_with(&root)
                && is_object(&resolved)
                && !files.contains(&resolved)
            {
                files.push(resolved);
            }
        }
    }
    Ok(files)
}

fn firstboot_overlays(
    board: &Amebad,
    sdk: &Path,
    overrides: &mut Overrides,
    staged: &dyn Fn(&Path, &Path, StagedFile) -> Result<()>,
) -> Result<()> {
    // Fast connect saves reconnect data to FAST_RECONNECT_DATA (0x105000),
    // inside the application bank; it also enables fast DHCP's read of that
    // sector. Other enabling lines sit inside examples this build leaves
    // disabled.
    let anchor = "/*For fast reconnection*/\n#define CONFIG_EXAMPLE_WLAN_FAST_CONNECT\t1\n";
    let text = replace_once(
        &pinned(sdk, PLATFORM_OPTS)?,
        anchor,
        &anchor.replace("\t1\n", "\t0\n"),
        SdkAnchor::FastConnect,
    )?;
    overlay(sdk, PLATFORM_OPTS, &text, overrides)?;

    for (core, source) in &board.mains {
        let relative = format!(
            "project/realtek_amebaD_va0_example/src/src_{}/main.c",
            core.dir()
        );
        write_if_changed(&sdk.join(&relative), &fs::read(source)?)?;
        staged(source, &sdk.join(&relative), StagedFile::FirstbootMain)?;
        overrides.insert(relative.clone(), digest(&sdk.join(&relative))?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIT: [&str; 8] = [
        "-c",
        "user.name=wellfed-test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "init.defaultBranch=main",
        "-c",
        "commit.gpgsign=false",
    ];

    /// Git on a scratch repository only: no system or global config, and
    /// the process's `GIT_*` variables gone (see `crate::git_command`).
    fn git_in(directory: &Path, args: &[&str]) -> String {
        let output = run(crate::git_command(directory)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(GIT)
            .args(args))
        .unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    struct Sdk {
        _temporary: tempfile::TempDir,
        path: PathBuf,
    }

    fn sdk() -> Sdk {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("sdk");
        let tracked = vendor_mode_only()
            .into_iter()
            .chain(vendor_rebuilt())
            .chain(managed_sdk_paths(Profile::Firstboot))
            .chain(["component/other.c".to_string()]);
        for name in tracked {
            let file = path.join(&name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, format!("pinned {name}")).unwrap();
        }
        git_in(&path, &["init", "-q"]);
        // Nothing is added or committed unless the repository is this one.
        let git_dir = PathBuf::from(git_in(&path, &["rev-parse", "--absolute-git-dir"]));
        assert!(
            git_dir.starts_with(temporary.path().canonicalize().unwrap()),
            "git init made {} outside the scratch directory",
            git_dir.display()
        );
        git_in(&path, &["add", "."]);
        git_in(&path, &["commit", "-q", "-m", "pinned"]);
        Sdk {
            _temporary: temporary,
            path,
        }
    }

    fn changes(sdk: &Sdk, profile: Profile) -> Vec<String> {
        unexpected_sdk_changes(&sdk.path, profile).unwrap()
    }

    #[test]
    fn the_retired_credentials_header_is_deleted_not_tolerated() {
        let sdk = sdk();
        let header = sdk.path.join(RETIRED_TRANSPORT_HEADER);
        fs::create_dir_all(header.parent().unwrap()).unwrap();
        fs::write(&header, "#define PASSWORD \"secret\"\n").unwrap();
        assert_eq!(
            changes(&sdk, Profile::Firstboot),
            vec![format!("?? {RETIRED_TRANSPORT_HEADER}")]
        );

        remove_retired_header(&sdk.path).unwrap();

        assert!(!header.exists());
        assert_eq!(changes(&sdk, Profile::Firstboot), Vec::<String>::new());
        remove_retired_header(&sdk.path).unwrap();
    }

    #[test]
    fn vendor_outputs_and_profile_overrides_are_allowed() {
        let sdk = sdk();
        for name in vendor_mode_only() {
            fs::set_permissions(sdk.path.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        for name in vendor_rebuilt()
            .into_iter()
            .chain(managed_sdk_paths(Profile::Firstboot))
        {
            fs::write(sdk.path.join(name), "regenerated").unwrap();
        }
        for name in vendor_untracked() {
            let file = sdk.path.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "vendor output").unwrap();
        }

        assert_eq!(changes(&sdk, Profile::Firstboot), Vec::<String>::new());
        check_sdk_worktree(&sdk.path, Profile::Firstboot).unwrap();
    }

    #[test]
    fn unexpected_changes_are_named_and_refused() {
        let sdk = sdk();
        let mode_only = vendor_mode_only().into_iter().next().unwrap();
        let helper = sdk.path.join(&mode_only);
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&helper, "changed helper").unwrap();
        fs::write(sdk.path.join("component/other.c"), "local edit").unwrap();
        fs::write(sdk.path.join("component/new.c"), "stray source").unwrap();
        let main = "project/realtek_amebaD_va0_example/src/src_lp/main.c";
        fs::write(sdk.path.join(main), "first-boot main").unwrap();

        let mut expected = vec![
            format!("M {mode_only}"),
            "M component/other.c".to_string(),
            "?? component/new.c".to_string(),
        ];
        expected.sort();
        assert_eq!(changes(&sdk, Profile::Firstboot), expected);
        // A first-boot main is only managed by the first-boot profile.
        assert!(changes(&sdk, Profile::Device).contains(&format!("M {main}")));

        let error = check_sdk_worktree(&sdk.path, Profile::Firstboot).unwrap_err();
        assert!(matches!(
            error,
            crate::Error::Sdk(SdkError::Worktree { changes, .. }) if changes == expected
        ));
    }

    #[test]
    fn staged_and_deleted_entries_are_refused() {
        let sdk = sdk();
        let managed: Vec<_> = managed_sdk_paths(Profile::Firstboot).into_iter().collect();
        fs::write(sdk.path.join(&managed[0]), "staged").unwrap();
        git_in(&sdk.path, &["add", &managed[0]]);
        fs::remove_file(sdk.path.join(&managed[1])).unwrap();

        let mut expected = vec![format!("M {}", managed[0]), format!("D {}", managed[1])];
        expected.sort();
        assert_eq!(changes(&sdk, Profile::Firstboot), expected);
    }

    #[test]
    fn the_tick_hook_and_name_length_are_set_and_only_they() {
        let pinned = "#define configMAX_TASK_NAME_LEN\t\t\t\t\t\t\t( 10 )\n\
                      #define configUSE_IDLE_HOOK\t\t\t\t\t\t\t\t0\n\
                      #define configUSE_TICK_HOOK\t\t\t\t\t\t\t\t0\n";
        let text = freertos_options(pinned).unwrap();
        assert!(text.contains("configUSE_TICK_HOOK\t\t\t\t\t\t\t\t1\n"));
        assert!(text.contains("configMAX_TASK_NAME_LEN\t\t\t\t\t\t\t( 16 )\n"));
        assert!(text.contains("configUSE_IDLE_HOOK\t\t\t\t\t\t\t\t0\n"));

        // A changed option line is refused, not patched elsewhere.
        let refused = |text: &str| match freertos_options(text) {
            Err(crate::Error::Sdk(SdkError::Anchor(anchor))) => anchor,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            refused("#define configUSE_TICK_HOOK 0\n"),
            SdkAnchor::TickHook
        );
        assert_eq!(
            refused(&pinned.replace("( 10 )", "( 12 )")),
            SdkAnchor::TaskNameLength
        );
        assert_eq!(refused(&pinned.repeat(2)), SdkAnchor::TickHook);
    }

    #[test]
    fn the_flash_speed_drops_to_50_mhz_and_the_read_mode_stays() {
        let pinned = "const u16 Flash_Speed = 0xFFFF;\nconst u16 Flash_ReadMode = 0xFFFF;\n";
        let text = flash_speed(pinned).unwrap();
        assert_eq!(
            text,
            "const u16 Flash_Speed = 0x0FFF;\nconst u16 Flash_ReadMode = 0xFFFF;\n"
        );

        // A changed or repeated option is refused, not patched elsewhere.
        for changed in [
            pinned.replace("0xFFFF;\nconst", "0x7FFF;\nconst"),
            pinned.repeat(2),
        ] {
            assert!(matches!(
                flash_speed(&changed),
                Err(crate::Error::Sdk(SdkError::Anchor(SdkAnchor::FlashSpeed)))
            ));
        }
    }

    #[test]
    fn an_option_change_recompiles_the_objects_built_beside_their_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let sdk = temporary.path().join("sdk");
        let asdk = sdk.join(project("project_hp/asdk"));
        let make = asdk.join("make/os");
        let component = sdk.join("component/common/file_system/littlefs");
        let image = asdk.join("image");
        for directory in [&make, &component, &image] {
            fs::create_dir_all(directory).unwrap();
        }
        let (tasks, lfs, source) = (
            make.join("tasks.o"),
            component.join("lfs.o"),
            component.join("lfs.c"),
        );
        let outside = temporary.path().join("outside.o");
        for file in [&tasks, &lfs, &source, &outside] {
            fs::write(file, "x").unwrap();
        }
        // The vendor recipe lists objects as paths relative to the KM4 project.
        fs::write(
            image.join("obj_list.txt"),
            format!(
                "\n{}\n../../../../../component/common/file_system/littlefs/lfs.o\n{}\n",
                tasks.display(),
                outside.display()
            ),
        )
        .unwrap();
        for options in [
            PLATFORM_OPTS.to_string(),
            project(HP_CONFIG),
            FREERTOS_CONFIG.to_string(),
        ] {
            let file = sdk.join(options);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "options").unwrap();
        }

        recompile_on_option_change(&sdk).unwrap();
        assert!(!tasks.exists() && !lfs.exists());
        assert!(source.exists(), "only objects go");
        assert!(outside.exists(), "nothing outside the worktree goes");

        // Unchanged options leave a rebuild's objects alone.
        fs::write(&lfs, "x").unwrap();
        recompile_on_option_change(&sdk).unwrap();
        assert!(lfs.exists());
        fs::write(sdk.join(FREERTOS_CONFIG), "changed").unwrap();
        recompile_on_option_change(&sdk).unwrap();
        assert!(!lfs.exists());
    }

    #[test]
    fn the_heap_leaves_the_trail_and_the_time_carry_above_it() {
        let pinned = "}\n\n__sram_end__ = ORIGIN(BD_RAM_NS) + LENGTH(BD_RAM_NS);\n";
        let text = retained_ram(pinned).unwrap();
        assert!(text.contains(
            "wellfed_time_carry = ORIGIN(BD_RAM_NS) + LENGTH(BD_RAM_NS) - 64;\n\
             wellfed_status_trail = wellfed_time_carry - 880;\n\
             __sram_end__ = wellfed_status_trail;"
        ));

        for changed in [pinned.replace("BD_RAM_NS)", "BD_RAM)"), pinned.repeat(2)] {
            assert!(matches!(
                retained_ram(&changed),
                Err(crate::Error::Sdk(SdkError::Anchor(SdkAnchor::SramEnd)))
            ));
        }
    }

    #[test]
    fn only_the_km4_link_wraps_the_trail_primitives() {
        let archive = Path::new("/build/rust-lib/libwellfed_km4.a");
        let km4 = link_additions(Core::Hp, archive);
        assert!(km4.starts_with("/build/rust-lib/libwellfed_km4.a -Wl,--wrap=save_and_cli "));
        for name in TRAIL_WRAPS {
            assert_eq!(
                km4.matches(&format!(" -Wl,--wrap={name}")).count(),
                1,
                "{name}"
            );
        }
        assert!(
            !km4.contains("rtw_enter_critical") && !km4.contains("rtw_exit_critical"),
            "the register-access sections stay unwrapped"
        );
        assert_eq!(
            link_additions(Core::Lp, archive),
            archive.display().to_string()
        );
    }

    #[test]
    fn the_fault_handler_prints_its_registers_before_the_backtrace() {
        let pinned = "\tu8 IsPstack = 0;\n\tcm_backtrace_fault((uint32_t)(IsPstack ? pstack : mstack), lr_value);\n\twhile(1);\n";
        let text = fault_registers(pinned).unwrap();
        let line = text.find("PLFLT1 cfsr=").unwrap();
        assert!(line < text.find("cm_backtrace_fault").unwrap());
        assert!(text.contains("0xE000ED28") && text.contains("0xE000ED38"));

        // A changed handler is refused, not patched elsewhere.
        for changed in ["\twhile(1);\n".to_string(), pinned.repeat(2)] {
            assert!(matches!(
                fault_registers(&changed),
                Err(crate::Error::Sdk(SdkError::Anchor(SdkAnchor::FaultHandler)))
            ));
        }
    }
}
