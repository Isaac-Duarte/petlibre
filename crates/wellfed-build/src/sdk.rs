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
use std::process::Command;

use wellfed_formats::layout::Layout;
use wellfed_formats::lock::SdkLock;
use regex::Regex;

use crate::audit::{hooks, pinmap};
use crate::credentials::{transport_header, wifi_header};
use crate::identity::{ENTRY, TRANSPORT_CREDENTIALS, WIFI_CREDENTIALS, firstboot_sources};
use crate::{Core, Error, PROJECT, Profile, Repo, Result, digest, git, require, revision, run};

const PLATFORM_OPTS: &str = "project/realtek_amebaD_va0_example/inc/inc_hp/platform_opts.h";
const WIFI_HEADER: &str = "component/common/example/wellfed_wifi_credentials.h";
const TRANSPORT_HEADER: &str = "component/common/example/wellfed_transport_credentials.h";
const OTA_HEADER: &str = "component/soc/realtek/amebad/fwlib/include/rtl8721d_ota.h";
const PINMAP: &str = "component/soc/realtek/amebad/fwlib/usrcfg/rtl8721dlp_pinmapcfg.c";
const UTILITIES_MAKEFILE: &str = "project_hp/asdk/make/utilities_example/Makefile";
const SRC_LP_MAKEFILE: &str = "project/realtek_amebaD_va0_example/src/src_lp/Makefile";
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
    if profile.model() {
        paths.extend([OTA_HEADER.to_string(), PINMAP.to_string()]);
    }
    if profile.rust() {
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
        ]);
    }
    if profile.firstboot() {
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
    let status = run(Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .current_dir(sdk))?
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
                    || (profile.rust() && (path == WIFI_HEADER || path == TRANSPORT_HEADER))
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

    let mut shown = unexpected[..unexpected.len().min(20)].join(", ");
    if unexpected.len() > 20 {
        shown.push_str(&format!(" (+{} more)", unexpected.len() - 20));
    }
    Err(Error::Refused(format!(
        "SDK worktree has changes outside this profile's overrides; left unchanged. \
         Review, then restore or recreate {}: {shown}",
        sdk.display()
    )))
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
    let output = run(Command::new("git")
        .args(["show", &format!("HEAD:{relative}")])
        .current_dir(sdk))?;
    String::from_utf8(output.stdout)
        .map_err(|_| Error::Refused(format!("Pinned SDK file is not UTF-8: {relative}")))
}

/// Replace the single `anchor` in `text`, refusing with `message` otherwise.
fn replace_once(text: &str, anchor: &str, with: &str, message: &str) -> Result<String> {
    require(text.matches(anchor).count() == 1, message)?;
    Ok(text.replacen(anchor, with, 1))
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
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
    fs::write(&path, text)?;
    overrides.insert(relative.to_string(), digest(&path)?);
    Ok(())
}

/// Create or check the profile's worktree and apply its overlays.
/// `expected` holds the input digests the identity was computed from;
/// staged copies of inputs must still match them.
pub(crate) fn prepare(
    repo: &Repo,
    profile: Profile,
    expected: &BTreeMap<String, String>,
) -> Result<(PathBuf, Overrides)> {
    let sdk = worktree(repo, profile, &repo.lock()?)?;

    let mut overrides = Overrides::new();
    let staged = |source: &Path, copy: &Path, message: &str| -> Result<()> {
        let name = crate::relative(source, &repo.root)?;
        require(
            expected.get(&name) == Some(&digest(copy)?),
            format!("{message}: {name}"),
        )
    };

    if profile.model() {
        ota_addresses(repo, &sdk, &mut overrides)?;
        let source = pinmap::overlay(&pinned(&sdk, PINMAP)?, &fs::read(repo.stock())?)?;
        overlay(&sdk, PINMAP, &source, &mut overrides)?;
    }
    if profile.rust() {
        rust_overlays(repo, &sdk, &mut overrides, &staged)?;
    }
    if profile.firstboot() {
        firstboot_overlays(repo, &sdk, &mut overrides, &staged)?;
    }
    Ok((sdk, overrides))
}

/// The profile's SDK worktree at the pinned revision, created on first
/// use and refused when it drifted, with both cores' toolchain links.
fn worktree(repo: &Repo, profile: Profile, lock: &SdkLock) -> Result<PathBuf> {
    let sdk = repo.output(profile).join("sdk");
    if !sdk.exists() {
        fs::create_dir_all(sdk.parent().unwrap_or(&sdk))?;
        run(Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&sdk)
            .arg(&lock.sdk.revision)
            .current_dir(repo.source()))?;
    }
    require(
        revision(&sdk)? == lock.sdk.revision,
        "Build worktree has an unexpected SDK revision",
    )?;
    check_sdk_worktree(&sdk, profile)?;

    // Both cores use the same verified toolchain, rather than downloading copies.
    let toolchains = repo.toolchains();
    for core in Core::BOTH {
        let link = sdk.join(project(&format!("project_{}/toolchain/linux", core.dir())));
        if link.is_symlink() && fs::canonicalize(&link).ok() == fs::canonicalize(&toolchains).ok() {
            continue;
        }
        require(
            fs::symlink_metadata(&link).is_err(),
            format!(
                "Unexpected toolchain path; left unchanged: {}",
                link.display()
            ),
        )?;
        std::os::unix::fs::symlink(&toolchains, &link)?;
    }
    Ok(sdk)
}

/// The Rust profiles' overlays: the application entry, both cores' link
/// rules and RAM sections, the flash-hook rename and the credential headers.
fn rust_overlays(
    repo: &Repo,
    sdk: &Path,
    overrides: &mut Overrides,
    staged: &dyn Fn(&Path, &Path, &str) -> Result<()>,
) -> Result<()> {
    let example = sdk.join("component/common/example");
    // The vendor makefiles name objects by basename and include old .d
    // files. Rebuild only our objects to avoid stale links.
    let object_root = sdk.join(project("project_hp/asdk"));
    for name in ["plaf103_flash_park", "plaf103_flash_hooks"] {
        remove_objects(&object_root.join("make/utilities_example"), name)?;
        remove_file(&object_root.join("build/ram").join(format!("{name}.o")))?;
    }

    let entry = repo.path(ENTRY);
    let destination = example.join("example_entry.c");
    fs::copy(&entry, &destination)?;
    staged(&entry, &destination, "Source changed while staging")?;

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
            &format!("{anchor} {}", archive.display()),
            "Pinned image2 link rule changed",
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
        let text = replace_once(
            &pinned(sdk, &relative)?,
            anchor,
            &format!("KEEP(*(.image2.ram.text.wellfed_park))\n\t\t{anchor}"),
            "Pinned RAM linker section changed",
        )?;
        overlay(sdk, &relative, &text, overrides)?;
    }
    let text = hooks::overlay(&pinned(sdk, hooks::SOURCE)?)?;
    overlay(sdk, hooks::SOURCE, &text, overrides)?;

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

    let wifi = repo.path(WIFI_CREDENTIALS);
    fs::write(sdk.join(WIFI_HEADER), wifi_header(&fs::read(&wifi)?)?)?;
    staged(&wifi, &wifi, "Wi-Fi credentials changed while staging")?;
    let transport = repo.path(TRANSPORT_CREDENTIALS);
    fs::write(
        sdk.join(TRANSPORT_HEADER),
        transport_header(&fs::read(&transport)?)?,
    )?;
    staged(
        &transport,
        &transport,
        "Transport credentials changed while staging",
    )?;
    Ok(())
}

/// Point the SDK's two OTA bank addresses at the stock layout's banks.
fn ota_addresses(repo: &Repo, sdk: &Path, overrides: &mut Overrides) -> Result<()> {
    let layout: Layout = wellfed_formats::load(&repo.layout())?;

    let mut text = pinned(sdk, OTA_HEADER)?;
    for (index, offset) in layout.ota_bank_offsets.iter().enumerate() {
        let address = layout.flash_mapping_base.0 + offset.0;
        let pattern = Regex::new(&format!(
            r"(#define\s+LS_IMG2_OTA{}_ADDR\s+)0x[0-9A-Fa-f]+",
            index + 1
        ))?;
        require(
            pattern.find_iter(&text).count() == 1,
            "Pinned SDK OTA address definition changed",
        )?;
        text = pattern
            .replace(&text, |c: &regex::Captures| {
                format!("{}0x{address:08X}", &c[1])
            })
            .into_owned();
    }
    overlay(sdk, OTA_HEADER, &text, overrides)
}

fn firstboot_overlays(
    repo: &Repo,
    sdk: &Path,
    overrides: &mut Overrides,
    staged: &dyn Fn(&Path, &Path, &str) -> Result<()>,
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
        "Pinned fast-connect option changed",
    )?;
    overlay(sdk, PLATFORM_OPTS, &text, overrides)?;

    // The vendor makefiles track no header dependencies, so objects compiled
    // under other platform options would be linked as they are. Recompile
    // every KM4 object (all untracked build outputs) when these options change.
    let stamp = sdk.parent().unwrap_or(sdk).join("hp-platform-opts.sha256");
    let options = &overrides[PLATFORM_OPTS];
    if fs::read_to_string(&stamp)
        .map(|s| s.trim().to_string())
        .ok()
        .as_ref()
        != Some(options)
    {
        let mut files = Vec::new();
        walk(&sdk.join(project("project_hp/asdk")), &mut files)?;
        for path in files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "o"))
        {
            fs::remove_file(path)?;
        }
        fs::write(&stamp, format!("{options}\n"))?;
    }

    for (core, source) in firstboot_sources(repo) {
        let relative = format!(
            "project/realtek_amebaD_va0_example/src/src_{}/main.c",
            core.dir()
        );
        fs::copy(&source, sdk.join(&relative))?;
        staged(
            &source,
            &sdk.join(&relative),
            "First-boot main changed while staging",
        )?;
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

    fn git_in(directory: &Path, args: &[&str]) {
        run(Command::new("git")
            .args(GIT)
            .args(args)
            .current_dir(directory))
        .unwrap();
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
            .chain(managed_sdk_paths(Profile::Plaf103Firstboot))
            .chain(["component/other.c".to_string()]);
        for name in tracked {
            let file = path.join(&name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, format!("pinned {name}")).unwrap();
        }
        git_in(&path, &["init", "-q"]);
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
    fn vendor_outputs_and_profile_overrides_are_allowed() {
        let sdk = sdk();
        for name in vendor_mode_only() {
            fs::set_permissions(sdk.path.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        for name in vendor_rebuilt()
            .into_iter()
            .chain(managed_sdk_paths(Profile::Plaf103Firstboot))
        {
            fs::write(sdk.path.join(name), "regenerated").unwrap();
        }
        for name in vendor_untracked() {
            let file = sdk.path.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "vendor output").unwrap();
        }

        assert_eq!(
            changes(&sdk, Profile::Plaf103Firstboot),
            Vec::<String>::new()
        );
        check_sdk_worktree(&sdk.path, Profile::Plaf103Firstboot).unwrap();
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
        assert_eq!(changes(&sdk, Profile::Plaf103Firstboot), expected);
        // A first-boot main is only managed by the first-boot profile.
        assert!(changes(&sdk, Profile::Plaf103).contains(&format!("M {main}")));

        let error = check_sdk_worktree(&sdk.path, Profile::Plaf103Firstboot).unwrap_err();
        assert!(error.to_string().contains("component/other.c"));
    }

    #[test]
    fn staged_and_deleted_entries_are_refused() {
        let sdk = sdk();
        let managed: Vec<_> = managed_sdk_paths(Profile::Plaf103Firstboot)
            .into_iter()
            .collect();
        fs::write(sdk.path.join(&managed[0]), "staged").unwrap();
        git_in(&sdk.path, &["add", &managed[0]]);
        fs::remove_file(sdk.path.join(&managed[1])).unwrap();

        let mut expected = vec![format!("M {}", managed[0]), format!("D {}", managed[1])];
        expected.sort();
        assert_eq!(changes(&sdk, Profile::Plaf103Firstboot), expected);
    }
}
