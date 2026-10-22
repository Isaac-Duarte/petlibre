//! One profile's build: cross-compile the Rust archives, run the vendor
//! recipes, check and audit the packaged image, and write `manifest.json`.
//!
//! The manifest is removed first and written last, so a failed build never
//! leaves one describing other outputs. Inputs, overlays, the SDK worktree,
//! the toolchain and the selected rustc are all checked again after
//! compilation, because the vendor recipes take minutes and nothing stops an
//! editor touching the tree meanwhile.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde_json::{Value, json};

use crate::audit::{self, Binutils, park};
use crate::environment::{process_environment, rust_compiler, rust_environment};
use crate::identity::{current_inputs, ota_public_key, source_identity};
use crate::sdk::{check_sdk_worktree, prepare, rust_archive};
use crate::toolchain::{toolchain_lock, verify_toolchain};
use crate::{Error, PROJECT, Profile, Repo, Result, Variant, digest, relative, require, text};

/// Cargo target and KM-core feature of each SDK core project.
pub const RUST_CORES: [(&str, &str, &str); 2] = [
    ("lp", "thumbv8m.base-none-eabi", "km0"),
    ("hp", "thumbv8m.main-none-eabihf", "km4"),
];

/// C ABI symbols every core archive must define. The core ABI is a C
/// interface for tests and later C consumers, retained in the archive, not
/// the image.
fn archive_symbols(core: &str) -> Vec<&'static str> {
    let mut symbols = if core == "hp" {
        vec![
            "wellfed_app_start",
            "wellfed_diagnostic_run",
            "wellfed_feed_controller_update",
            "wellfed_sensor_inputs_update",
            "wellfed_adc_scale_16",
            "wellfed_motor_gate_request",
            "wellfed_feedback_events_process",
            "wellfed_tick_clock_read",
            "wellfed_flash_coord_init",
            "wellfed_flash_coord_request",
            "wellfed_flash_coord_parked",
            "wellfed_flash_coord_released",
            "wellfed_flash_coord_outputs_blocked",
            "wellfed_feed_journal_open",
            "wellfed_feed_journal_create",
            "wellfed_feed_journal_read",
            "wellfed_feed_journal_storage",
        ]
    } else {
        Vec::new()
    };
    let mut park = park::SYMBOLS.to_vec();
    park.sort();
    symbols.extend(park);
    symbols.extend([
        "FLASH_Write_Lock",
        "FLASH_Write_Unlock",
        "FLASH_Write_IPC_Int",
    ]);
    symbols
}

fn core_name(core: &str) -> &'static str {
    if core == "lp" { "KM0" } else { "KM4" }
}

/// The cargo environment of the archive builds: the checked environment
/// plus what `wellfed-ffi`'s build script reads.
fn cargo_environment(
    environment: &BTreeMap<String, String>,
    sdk: &Path,
    toolchain: &Path,
    identity: &str,
    ota_key: Option<&str>,
) -> BTreeMap<String, String> {
    let mut environment = environment.clone();
    environment.insert("WELLFED_BUILD_ID".into(), identity.into());
    environment.insert("WELLFED_SDK_DIR".into(), sdk.display().to_string());
    environment.insert(
        "WELLFED_NEWLIB_INCLUDE".into(),
        toolchain
            .join("arm-none-eabi/include")
            .display()
            .to_string(),
    );
    environment.remove("WELLFED_OTA_PUBLIC_KEY");
    if let Some(key) = ota_key {
        environment.insert("WELLFED_OTA_PUBLIC_KEY".into(), key.into());
    }
    environment
}

/// Cross-compile one firmware archive per core and stage it beside the SDK.
fn build_rust(
    repo: &Repo,
    sdk: &Path,
    log: &File,
    environment: &BTreeMap<String, String>,
    variant: Option<Variant>,
) -> Result<Vec<(String, PathBuf)>> {
    let target_dir = sdk.parent().unwrap_or(sdk).join("rust-target");
    let mut archives = Vec::new();
    for (core, triple, feature) in RUST_CORES {
        let feature = match variant {
            Some(variant) if core == "hp" => variant.name(),
            _ => feature,
        };
        let status = Command::new("cargo")
            .args([
                "build",
                "--locked",
                "--profile",
                "firmware",
                "-p",
                "wellfed-ffi",
            ])
            .args(["--target", triple, "--features", feature, "--target-dir"])
            .arg(&target_dir)
            .current_dir(&repo.root)
            .env_clear()
            .envs(environment)
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?)
            .status()?;
        require(status.success(), "Rust firmware build failed")?;

        let staged = rust_archive(sdk, core);
        fs::create_dir_all(staged.parent().unwrap_or(sdk))?;
        fs::copy(
            target_dir.join(triple).join("firmware/libwellfed_ffi.a"),
            &staged,
        )?;
        archives.push((core_name(core).to_string(), staged));
    }
    Ok(archives)
}

/// Defined symbol names in `path`, from `nm`.
fn defined(nm: &Path, path: &Path, fields: usize) -> Result<Vec<String>> {
    Ok(text(Command::new(nm).arg("--defined-only").arg(path))?
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            (if fields == 0 {
                !parts.is_empty()
            } else {
                parts.len() == fields
            })
            .then(|| parts[parts.len() - 1].to_string())
        })
        .collect())
}

/// Print a bounded excerpt of a failed SDK build: SDK command lines can span
/// tens of thousands of characters, so the full log stays private.
fn report_failure(log: &Path) {
    static ERROR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"error:|undefined reference|No rule to make target|Error [1-9]").unwrap()
    });
    let text = String::from_utf8_lossy(&fs::read(log).unwrap_or_default()).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let errors: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| ERROR.is_match(l))
        .collect();
    let shown = if errors.is_empty() {
        &lines[lines.len().saturating_sub(12)..]
    } else {
        &errors[..]
    };
    for line in &shown[shown.len().saturating_sub(20)..] {
        println!("{}", bounded(line, 600));
    }
}

fn bounded(line: &str, limit: usize) -> String {
    if line.chars().count() <= limit {
        line.to_string()
    } else {
        format!("{}...", line.chars().take(limit - 3).collect::<String>())
    }
}

/// Compiler warnings or errors in the sources this builder staged.
fn staged_diagnostics(log: &Path, profile: Profile) -> Vec<String> {
    static ENTRY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"example_entry\.c:.*(?:warning|error):").unwrap());
    static MAINS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/src_(?:lp|hp)/main\.c:.*(?:warning|error):").unwrap());
    let text = String::from_utf8_lossy(&fs::read(log).unwrap_or_default()).into_owned();
    let mut found: Vec<String> = text
        .lines()
        .filter(|l| ENTRY.is_match(l))
        .map(str::to_string)
        .collect();
    if profile.firstboot() {
        found.extend(
            text.lines()
                .filter(|l| MAINS.is_match(l))
                .map(str::to_string),
        );
    }
    found
}

/// The OTA container must hold exactly the combined image, with its checksum.
fn check_container(ota: &[u8], combined: &[u8]) -> Result<()> {
    require(ota.len() >= 32, "OTA container is shorter than its header")?;
    let word = |i: usize| u32::from_le_bytes(ota[i * 4..i * 4 + 4].try_into().unwrap());
    let (version, count, magic, entry, checksum, size, offset, address) = (
        word(0),
        word(1),
        word(2),
        word(3),
        word(4),
        word(5),
        word(6),
        word(7),
    );
    require(
        (version, count, magic, entry, offset, address)
            == (
                0xffff_ffff,
                1,
                u32::from_le_bytes(*b"OTA1"),
                24,
                32,
                0xffff_ffff,
            )
            && size as usize == combined.len()
            && &ota[32..] == combined,
        "OTA container does not match the combined image",
    )?;
    let sum = combined.iter().fold(0u32, |s, b| s.wrapping_add(*b as u32));
    require(sum == checksum, "OTA container checksum does not match")
}

pub fn build(
    repo: &Repo,
    profile: Profile,
    toolchain: &Path,
    variant: Option<Variant>,
) -> Result<()> {
    require(
        variant.is_none() || profile.firstboot(),
        "Image variants apply to plaf103-firstboot only",
    )?;
    let lock = repo.lock()?;
    let output = repo.output(profile);
    fs::create_dir_all(&output)?;
    match fs::remove_file(output.join("manifest.json")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }

    let (rust_env, compiler) = if profile.rust() {
        let environment = rust_environment(&repo.root, process_environment())?;
        let compiler = rust_compiler(repo, &environment)?;
        (Some(environment), Some(compiler))
    } else {
        (None, None)
    };
    let expected = current_inputs(repo, profile)?;
    let ota_key = ota_public_key(repo, profile)?;
    let identity = source_identity(profile, &expected, variant, ota_key.as_deref())?;
    if profile.model() {
        audit::layout::audit(&repo.stock(), &repo.stock(), &repo.layout())
            .map_err(|e| Error::Refused(format!("Offline layout audit failed: {e}")))?;
    }

    let (sdk, overrides) = prepare(repo, profile, &expected)?;
    let output = sdk.parent().unwrap_or(&sdk).to_path_buf();
    let log_path = output.join("build.log");
    let binutils = Binutils {
        dir: toolchain.join("bin"),
    };
    println!(
        "Building {}{}; log: {}",
        profile.name(),
        variant
            .map(|v| format!(" variant {}", v.name()))
            .unwrap_or_default(),
        log_path.display()
    );

    let log = File::create(&log_path)?;
    let mut archives = Vec::new();
    if let Some(environment) = &rust_env {
        println!("Building Rust firmware archives...");
        let environment =
            cargo_environment(environment, &sdk, toolchain, &identity, ota_key.as_deref());
        archives = build_rust(repo, &sdk, &log, &environment, variant).map_err(|_| {
            Error::Refused(format!(
                "Rust firmware build failed; see {}",
                log_path.display()
            ))
        })?;
    }
    for core in ["lp", "hp"] {
        println!("Building {}...", core_name(core));
        // The vendor recipes aren't safe for parallel make; keep their default order.
        let status = Command::new("make")
            .arg("all")
            .current_dir(sdk.join(PROJECT).join(format!("project_{core}")))
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?)
            .status()?;
        if !status.success() {
            report_failure(&log_path);
            return Err(Error::Refused(format!(
                "SDK build failed; see {}",
                log_path.display()
            )));
        }
    }
    drop(log);

    let diagnostics = staged_diagnostics(&log_path, profile);
    if !diagnostics.is_empty() {
        for line in diagnostics.iter().take(20) {
            println!("{}", line.chars().take(600).collect::<String>());
        }
        return Err(Error::Refused(format!(
            "Staged firmware source has compiler diagnostics; see {}",
            log_path.display()
        )));
    }

    let mut artifacts = Vec::new();
    for (core, name) in [
        ("lp", "km0_image2_all.bin"),
        ("hp", "km4_image2_all.bin"),
        ("hp", "km0_km4_image2.bin"),
        ("hp", "OTA_All.bin"),
    ] {
        let path = sdk
            .join(PROJECT)
            .join(format!("project_{core}/asdk/image"))
            .join(name);
        let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        require(
            path.is_file() && bytes > 0,
            format!("Missing build artifact: {}", path.display()),
        )?;
        artifacts.push(json!({"path": relative(&path, &output)?, "bytes": bytes,
                              "sha256": digest(&path)?}));
    }

    let image_directory = sdk.join(PROJECT).join("project_hp/asdk/image");
    let combined = fs::read(image_directory.join("km0_km4_image2.bin"))?;
    let ota = fs::read(image_directory.join("OTA_All.bin"))?;
    check_container(&ota, &combined)?;
    let banner = contains(&combined, b"Wellfed local firmware: Rust bringup");
    require(
        banner == profile.rust(),
        "Linked image does not match the requested application profile",
    )?;
    require(
        !profile.rust()
            || contains(
                &combined,
                format!("PLFW1 start build={identity} ").as_bytes(),
            ),
        "Linked image is missing the source-bound startup identity",
    )?;

    let mut required_symbols: Vec<&str> = Vec::new();
    let mut compiled_archives = Vec::new();
    if profile.rust() {
        // The linked KM4 image must contain the Rust application entry and
        // the SDK interposer; park routines are checked by the RAM park audit.
        required_symbols = vec![
            "wellfed_app_start",
            "FLASH_Write_Lock",
            "FLASH_Write_Unlock",
            "FLASH_Write_IPC_Int",
        ];
        let names = defined(&binutils.nm(), &image_directory.join("target_img2.axf"), 0)?;
        require(
            required_symbols
                .iter()
                .all(|s| names.iter().any(|n| n == s)),
            "Linked image is missing required application/core symbols",
        )?;

        for ((core, triple, _), (_, archive)) in RUST_CORES.iter().zip(&archives) {
            let names = defined(&binutils.nm(), archive, 3)?;
            let required = archive_symbols(core);
            let mut missing: Vec<&str> = required
                .iter()
                .filter(|s| !names.iter().any(|n| n == *s))
                .copied()
                .collect();
            missing.sort();
            require(
                missing.is_empty(),
                format!(
                    "Rust archive is missing expected symbols: {}",
                    missing.join(", ")
                ),
            )?;
            compiled_archives.push(json!({"core": core_name(core), "target": triple,
                                          "path": relative(archive, &output)?,
                                          "sha256": digest(archive)?, "symbols": required,
                                          "hardware_activated": false}));
        }
    }

    require(
        current_inputs(repo, profile)? == expected,
        "Sources changed during the build; rerun before trusting these outputs",
    )?;
    for (path, checksum) in &overrides {
        require(
            &digest(&sdk.join(path))? == checksum,
            "Generated SDK layout override changed during compilation",
        )?;
    }
    check_sdk_worktree(&sdk, profile)?;
    let pins = toolchain_lock(repo, &lock)?;
    verify_toolchain(toolchain, &pins)?;
    if let Some(environment) = &rust_env {
        require(
            Some(rust_compiler(repo, environment)?) == compiler,
            "Selected rustc changed during the build",
        )?;
    }

    let stock = fs::read(repo.stock()).ok();
    let mut audits = json!({});
    if profile.model() {
        let stock = stock.as_deref().unwrap_or_default();
        audits["layout_audit"] = audit::layout::audit(
            &image_directory.join("OTA_All.bin"),
            &repo.stock(),
            &repo.layout(),
        )
        .map_err(|e| Error::Refused(format!("Offline layout audit failed: {e}")))?;
        audits["km0_pinmap_audit"] =
            audit::pinmap::audit_linked_table(&ota, audit::pmap_address(&sdk, &binutils)?, stock)?;
    }
    if profile.rust() {
        audits["ram_park_audit"] = park::audit(&sdk, &binutils, &archives)?;
        audits["sdk_hook_audit"] = audit::hooks::audit(&sdk, &binutils, &archives)?;
    }
    if profile.firstboot() {
        audits["firstboot_startup_audit"] = audit::mains::audit(&sdk, &binutils)?;
        audits["wifi_link_audit"] = audit::writes::audit(&sdk, &binutils)?;
        audits["stack_audit"] = audit::stack::audit(
            &image_directory.join("target_img2.axf"),
            &binutils.objdump(),
            &fs::read_to_string(repo.path(audit::stack::APP_SOURCE))?,
        )?;
    }
    let audit = |key: &str| audits.get(key).cloned().unwrap_or(Value::Null);

    let rust = profile.rust();
    let firstboot = profile.firstboot();
    let when = |condition: bool, value: &str| condition.then(|| value.to_string());
    let manifest = json!({
        "profile": profile.name(),
        "sdk_revision": lock.sdk_revision,
        "startup_identity": when(rust, &identity),
        "startup_output_policy": when(rust, "stop-only"),
        // The FW04 pulse probe is gone; startup only drives the indicators LOW.
        "startup_led_policy": when(rust, "low-only-v1"),
        "startup_gpio_routing_policy": when(rust, "read-only-v3"),
        "startup_shared_pad_policy": when(firstboot, "before-km4-v3"),
        "startup_gpio_clock_policy": when(rust, "explicit-enable-v1"),
        "startup_flash_handshake_policy": null,
        "startup_adc_policy": null,
        "startup_wifi_policy": when(firstboot, "fw07-dns-v1"),
        "startup_transport_policy": when(firstboot, "fw08-transport-v1"),
        "startup_optics_policy": when(firstboot && variant.is_none(), "fw13-optics-v2"),
        "startup_motion_policy": variant.and_then(Variant::motion_policy),
        "startup_trial_policy": variant.and_then(Variant::trial_policy),
        "variant": variant.map(Variant::name),
        "startup_update_policy": ota_key.as_ref().map(|_| "fw10-recovery-v1"),
        "ota_public_key": ota_key,
        "toolchain_sha256": lock.toolchain_sha256,
        "built_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        "inputs": expected,
        "sdk_overrides": overrides,
        "compiled_archives": compiled_archives,
        "rust_toolchain": compiler.as_ref().map(|c| c["version"].clone()),
        "rust_compiler": compiler,
        "toolchain_files": pins,
        "layout_audit": audit("layout_audit"),
        "km0_pinmap_audit": audit("km0_pinmap_audit"),
        "ram_park_audit": audit("ram_park_audit"),
        "sdk_hook_audit": audit("sdk_hook_audit"),
        "firstboot_startup_audit": audit("firstboot_startup_audit"),
        "wifi_link_audit": audit("wifi_link_audit"),
        "stack_audit": audit("stack_audit"),
        "bootloader_replacement_supported": false,
        "artifacts": artifacts,
        "checks": {"ota_payload_matches": true, "ota_checksum_matches": true,
                   "bringup_banner_present": banner,
                   "required_core_symbols": required_symbols,
                   "source_snapshot_unchanged": true,
                   "sdk_worktree_within_allowlist": true,
                   "staged_sources_without_diagnostics": true},
        "device_compatibility": "Unverified SDK development image; desktop build only",
    });

    let path = output.join("manifest.json");
    fs::write(&path, serde_json::to_string_pretty(&manifest)? + "\n")?;
    println!("Build complete; manifest: {}", path.display());
    Ok(())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Re-run a finished build's audits for its review: the archives come from
/// its manifest, everything else from the build directory and the checkout.
pub fn audit_build(repo: &Repo, build: &Path) -> Result<Value> {
    let manifest: Value = serde_json::from_slice(&fs::read(build.join("manifest.json"))?)?;
    let objects: Vec<(String, PathBuf)> = manifest["compiled_archives"]
        .as_array()
        .map(|archives| {
            archives
                .iter()
                .filter_map(|a| {
                    Some((
                        a["core"].as_str()?.to_string(),
                        build.join(a["path"].as_str()?),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    audit::linked(repo, build, &objects)
}
