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
use petlibre_formats::lock::SdkLock;
use petlibre_formats::manifest::{
    Artifact, Audits, BuildChecks, BuildManifest, CompiledArchive, RustCompiler,
};
use regex::Regex;

use crate::audit::{self, Archives, Binutils, park, word};
use crate::environment::{Environment, process_environment, rust_compiler, rust_environment};
use crate::identity::{current_inputs, ota_public_key, source_identity};
use crate::sdk::{Overrides, check_sdk_worktree, prepare, rust_archive};
use crate::toolchain::{toolchain_lock, verify_toolchain};
use crate::{
    Core, Error, PROJECT, Profile, Repo, Result, Variant, digest, relative, require, text,
};

/// C ABI symbols every core archive must define. The core ABI is a C
/// interface for tests and later C consumers, retained in the archive, not
/// the image.
fn archive_symbols(core: Core) -> Vec<&'static str> {
    let mut symbols = match core {
        Core::Hp => vec![
            "petlibre_app_start",
            "petlibre_diagnostic_run",
            "petlibre_feed_controller_update",
            "petlibre_sensor_inputs_update",
            "petlibre_adc_scale_16",
            "petlibre_motor_gate_request",
            "petlibre_feedback_events_process",
            "petlibre_tick_clock_read",
            "petlibre_flash_coord_init",
            "petlibre_flash_coord_request",
            "petlibre_flash_coord_parked",
            "petlibre_flash_coord_released",
            "petlibre_flash_coord_outputs_blocked",
            "petlibre_feed_journal_open",
            "petlibre_feed_journal_create",
            "petlibre_feed_journal_read",
            "petlibre_feed_journal_storage",
        ],
        Core::Lp => Vec::new(),
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

/// The cargo environment of the archive builds: the checked environment
/// plus what `petlibre-ffi`'s build script reads.
fn cargo_environment(
    environment: &Environment,
    sdk: &Path,
    toolchain: &Path,
    identity: &str,
    ota_key: Option<&str>,
) -> Environment {
    let mut environment = environment.clone();
    environment.insert("PETLIBRE_BUILD_ID".into(), identity.into());
    environment.insert("PETLIBRE_SDK_DIR".into(), sdk.display().to_string());
    environment.insert(
        "PETLIBRE_NEWLIB_INCLUDE".into(),
        toolchain
            .join("arm-none-eabi/include")
            .display()
            .to_string(),
    );
    environment.remove("PETLIBRE_OTA_PUBLIC_KEY");
    if let Some(key) = ota_key {
        environment.insert("PETLIBRE_OTA_PUBLIC_KEY".into(), key.into());
    }
    environment
}

/// Cross-compile one firmware archive per core and stage it beside the SDK.
fn build_rust(
    repo: &Repo,
    sdk: &Path,
    log: &File,
    environment: &Environment,
    variant: Option<Variant>,
) -> Result<Vec<(Core, PathBuf)>> {
    let target_dir = sdk.parent().unwrap_or(sdk).join("rust-target");
    let mut archives = Vec::new();
    for core in Core::BOTH {
        let feature = match variant {
            Some(variant) if core == Core::Hp => variant.name(),
            _ => core.feature(),
        };
        let status = Command::new("cargo")
            .args([
                "build",
                "--locked",
                "--profile",
                "firmware",
                "-p",
                "petlibre-ffi",
            ])
            .args(["--target", core.target(), "--features", feature])
            .arg("--target-dir")
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
            target_dir
                .join(core.target())
                .join("firmware/libpetlibre_ffi.a"),
            &staged,
        )?;
        archives.push((core, staged));
    }
    Ok(archives)
}

/// Which `nm` lines name a symbol.
#[derive(Debug, Clone, Copy)]
enum NmLines {
    /// Any non-empty line (an image's listing).
    Any,
    /// Lines of exactly address, kind and name (an archive's members).
    Defined,
}

/// Defined symbol names in `path`, from `nm`.
fn defined(nm: &Path, path: &Path, lines: NmLines) -> Result<Vec<String>> {
    Ok(text(Command::new(nm).arg("--defined-only").arg(path))?
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let named = match lines {
                NmLines::Any => !parts.is_empty(),
                NmLines::Defined => parts.len() == 3,
            };
            named.then(|| parts[parts.len() - 1].to_string())
        })
        .collect())
}

/// Print a bounded excerpt of a failed SDK build: SDK command lines can span
/// tens of thousands of characters, so the full log stays private.
fn report_failure(log: &Path) {
    static ERROR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"error:|undefined reference|No rule to make target|Error [1-9]")
            .expect("error pattern")
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
    static ENTRY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"example_entry\.c:.*(?:warning|error):").expect("entry pattern")
    });
    static MAINS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"/src_(?:lp|hp)/main\.c:.*(?:warning|error):").expect("mains pattern")
    });
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
    let header: Vec<u64> = (0..8).filter_map(|i| word(ota, i * 4)).collect();
    let [
        version,
        count,
        magic,
        entry,
        checksum,
        size,
        offset,
        address,
    ] = header[..]
    else {
        return Err(Error::Refused(
            "OTA container is shorter than its header".into(),
        ));
    };
    require(
        (version, count, magic, entry, offset, address)
            == (
                0xffff_ffff,
                1,
                u64::from(u32::from_le_bytes(*b"OTA1")),
                24,
                32,
                0xffff_ffff,
            )
            && size == combined.len() as u64
            && &ota[32..] == combined,
        "OTA container does not match the combined image",
    )?;

    let sum = combined
        .iter()
        .fold(0u32, |s, b| s.wrapping_add(u32::from(*b)));
    require(
        u64::from(sum) == checksum,
        "OTA container checksum does not match",
    )
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The packaged artifacts the manifest records: each core's image, the
/// combined image and the OTA container.
const ARTIFACTS: [(Core, &str); 4] = [
    (Core::Lp, "km0_image2_all.bin"),
    (Core::Hp, "km4_image2_all.bin"),
    (Core::Hp, "km0_km4_image2.bin"),
    (Core::Hp, "OTA_All.bin"),
];

/// The Rust compiler a Rust profile builds with, resolved before and
/// compared after compilation.
struct Rust {
    environment: Environment,
    compiler: RustCompiler,
}

/// One profile's build in its SDK worktree.
struct Build<'a> {
    repo: &'a Repo,
    profile: Profile,
    variant: Option<Variant>,
    toolchain: &'a Path,
    sdk: PathBuf,
    /// The profile's output directory, which holds the worktree.
    output: PathBuf,
    log: PathBuf,
    binutils: Binutils,
}

impl Build<'_> {
    fn image_directory(&self) -> PathBuf {
        self.sdk.join(PROJECT).join("project_hp/asdk/image")
    }

    /// The Rust archives (for a Rust profile), then both cores' vendor
    /// recipes, refusing any compiler diagnostic in the staged sources.
    fn compile(
        &self,
        rust: Option<&Rust>,
        identity: &str,
        ota_key: Option<&str>,
    ) -> Result<Vec<(Core, PathBuf)>> {
        let log = File::create(&self.log)?;
        let logged = |stage| Error::Logged {
            stage,
            log: self.log.clone(),
        };

        let mut archives = Vec::new();
        if let Some(rust) = rust {
            println!("Building Rust firmware archives...");
            let environment = cargo_environment(
                &rust.environment,
                &self.sdk,
                self.toolchain,
                identity,
                ota_key,
            );
            archives = build_rust(self.repo, &self.sdk, &log, &environment, self.variant)
                .map_err(|_| logged("Rust firmware build failed"))?;
        }

        for core in Core::BOTH {
            println!("Building {}...", core.name());
            // The vendor recipes aren't safe for parallel make; keep their default order.
            let status = Command::new("make")
                .arg("all")
                .current_dir(
                    self.sdk
                        .join(PROJECT)
                        .join(format!("project_{}", core.dir())),
                )
                .env_remove("MAKEFLAGS")
                .env_remove("MFLAGS")
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log.try_clone()?)
                .status()?;
            if !status.success() {
                report_failure(&self.log);
                return Err(logged("SDK build failed"));
            }
        }
        drop(log);

        let diagnostics = staged_diagnostics(&self.log, self.profile);
        if !diagnostics.is_empty() {
            for line in diagnostics.iter().take(20) {
                println!("{}", line.chars().take(600).collect::<String>());
            }
            return Err(logged("Staged firmware source has compiler diagnostics"));
        }
        Ok(archives)
    }

    fn artifacts(&self) -> Result<Vec<Artifact>> {
        let mut artifacts = Vec::new();
        for (core, name) in ARTIFACTS {
            let path = self
                .sdk
                .join(PROJECT)
                .join(format!("project_{}/asdk/image", core.dir()))
                .join(name);
            let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            require(
                path.is_file() && bytes > 0,
                format!("Missing build artifact: {}", path.display()),
            )?;
            artifacts.push(Artifact {
                path: relative(&path, &self.output)?,
                bytes,
                sha256: digest(&path)?,
            });
        }
        Ok(artifacts)
    }

    /// The OTA container holds the combined image, which runs the requested
    /// application and embeds `identity`. Returns the OTA and whether the
    /// Rust banner is present.
    fn check_image(&self, identity: &str) -> Result<(Vec<u8>, bool)> {
        let combined = fs::read(self.image_directory().join("km0_km4_image2.bin"))?;
        let ota = fs::read(self.image_directory().join("OTA_All.bin"))?;
        check_container(&ota, &combined)?;

        let rust = self.profile.rust();
        let banner = contains(&combined, b"Petlibre local firmware: Rust bringup");
        require(
            banner == rust,
            "Linked image does not match the requested application profile",
        )?;
        require(
            !rust
                || contains(
                    &combined,
                    format!("PLFW1 start build={identity} ").as_bytes(),
                ),
            "Linked image is missing the source-bound startup identity",
        )?;
        Ok((ota, banner))
    }

    /// The linked KM4 image must contain the Rust application entry and the
    /// SDK interposer (park routines are checked by the RAM park audit), and
    /// each archive its C ABI. Returns the image symbols checked and the
    /// archives' manifest entries.
    fn check_symbols(&self, archives: &Archives) -> Result<(Vec<String>, Vec<CompiledArchive>)> {
        let required = [
            "petlibre_app_start",
            "FLASH_Write_Lock",
            "FLASH_Write_Unlock",
            "FLASH_Write_IPC_Int",
        ];
        let image = self.image_directory().join("target_img2.axf");
        let names = defined(&self.binutils.nm(), &image, NmLines::Any)?;
        require(
            required.iter().all(|s| names.iter().any(|n| n == s)),
            "Linked image is missing required application/core symbols",
        )?;

        let mut compiled = Vec::new();
        for (core, archive) in archives {
            let names = defined(&self.binutils.nm(), archive, NmLines::Defined)?;
            let symbols = archive_symbols(*core);
            let mut missing: Vec<&str> = symbols
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

            compiled.push(CompiledArchive {
                core: core.name().into(),
                target: core.target().into(),
                path: relative(archive, &self.output)?,
                sha256: digest(archive)?,
                symbols: symbols.iter().map(|s| s.to_string()).collect(),
                hardware_activated: false,
            });
        }
        Ok((required.map(String::from).to_vec(), compiled))
    }

    /// Nothing the build read changed while it compiled. Returns the
    /// toolchain's file pins.
    fn recheck(
        &self,
        expected: &BTreeMap<String, String>,
        overrides: &Overrides,
        lock: &SdkLock,
        rust: Option<&Rust>,
    ) -> Result<BTreeMap<String, String>> {
        require(
            current_inputs(self.repo, self.profile)? == *expected,
            "Sources changed during the build; rerun before trusting these outputs",
        )?;
        for (path, checksum) in overrides {
            require(
                &digest(&self.sdk.join(path))? == checksum,
                "Generated SDK layout override changed during compilation",
            )?;
        }
        check_sdk_worktree(&self.sdk, self.profile)?;

        let pins = toolchain_lock(self.repo, lock)?;
        verify_toolchain(self.toolchain, &pins)?;
        if let Some(rust) = rust {
            require(
                rust_compiler(self.repo, &rust.environment)? == rust.compiler,
                "Selected rustc changed during the build",
            )?;
        }
        Ok(pins)
    }

    /// The profile's linked-image audits, as the manifest records them.
    fn audits(&self, ota: &[u8], archives: &Archives) -> Result<Audits> {
        let repo = self.repo;
        let (sdk, binutils) = (&self.sdk, &self.binutils);

        let mut audits = Audits::default();
        if self.profile.model() {
            let stock = fs::read(repo.stock()).unwrap_or_default();
            let layout = audit::layout::audit(
                &self.image_directory().join("OTA_All.bin"),
                &repo.stock(),
                &repo.layout(),
            )
            .map_err(|e| Error::Layout(Box::new(e)))?;
            audits.layout_audit = recorded(&layout)?;
            audits.km0_pinmap_audit = recorded(&audit::pinmap::audit_linked_table(
                ota,
                audit::pmap_address(sdk, binutils)?,
                &stock,
            )?)?;
        }
        if self.profile.rust() {
            audits.ram_park_audit = recorded(&park::audit(sdk, binutils, archives)?)?;
            audits.sdk_hook_audit = recorded(&audit::hooks::audit(sdk, binutils, archives)?)?;
        }
        if self.profile.firstboot() {
            audits.firstboot_startup_audit = recorded(&audit::mains::audit(sdk, binutils)?)?;
            audits.wifi_link_audit = recorded(&audit::writes::audit(sdk, binutils)?)?;
            audits.stack_audit = recorded(&audit::stack::audit(
                &self.image_directory().join("target_img2.axf"),
                &binutils.objdump(),
                &fs::read_to_string(repo.path(audit::stack::APP_SOURCE))?,
            )?)?;
        }
        Ok(audits)
    }
}

/// An audit as the manifest records it: the same serialization
/// `petlibre-build audit` prints, which the frozen review compares.
fn recorded(audit: &impl serde::Serialize) -> Result<Option<serde_json::Value>> {
    Ok(Some(serde_json::to_value(audit)?))
}

/// Remove a previous build's manifest before anything else, so a failed
/// build never leaves one describing other outputs.
fn remove_manifest(output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    match fs::remove_file(output.join("manifest.json")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
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
    remove_manifest(&repo.output(profile))?;

    let rust = if profile.rust() {
        let environment = rust_environment(&repo.root, process_environment())?;
        let compiler = rust_compiler(repo, &environment)?;
        Some(Rust {
            environment,
            compiler,
        })
    } else {
        None
    };
    let expected = current_inputs(repo, profile)?;
    let ota_key = ota_public_key(repo, profile)?;
    let identity = source_identity(profile, &expected, variant, ota_key.as_deref())?;
    if profile.model() {
        audit::layout::audit(&repo.stock(), &repo.stock(), &repo.layout())
            .map_err(|e| Error::Layout(Box::new(e)))?;
    }

    let (sdk, overrides) = prepare(repo, profile, &expected)?;
    let output = sdk.parent().unwrap_or(&sdk).to_path_buf();
    let build = Build {
        repo,
        profile,
        variant,
        toolchain,
        log: output.join("build.log"),
        output,
        sdk,
        binutils: Binutils {
            dir: toolchain.join("bin"),
        },
    };
    println!(
        "Building {}{}; log: {}",
        profile.name(),
        variant
            .map(|v| format!(" variant {}", v.name()))
            .unwrap_or_default(),
        build.log.display()
    );

    let archives = build.compile(rust.as_ref(), &identity, ota_key.as_deref())?;
    let artifacts = build.artifacts()?;
    let (ota, banner) = build.check_image(&identity)?;
    let (required_symbols, compiled_archives) = if profile.rust() {
        build.check_symbols(&archives)?
    } else {
        Default::default()
    };
    let pins = build.recheck(&expected, &overrides, &lock, rust.as_ref())?;
    let audits = build.audits(&ota, &archives)?;

    let firstboot = profile.firstboot();
    let when = |condition: bool, value: &str| condition.then(|| value.to_string());
    let rust_build = profile.rust();
    let compiler = rust.map(|rust| rust.compiler);
    let manifest = BuildManifest {
        profile: profile.name().into(),
        sdk_revision: lock.sdk.revision,
        startup_identity: when(rust_build, &identity),
        startup_output_policy: when(rust_build, "stop-only"),
        // The FW04 pulse probe is gone; startup only drives the indicators LOW.
        startup_led_policy: when(rust_build, "low-only-v1"),
        startup_gpio_routing_policy: when(rust_build, "read-only-v3"),
        startup_shared_pad_policy: when(firstboot, "before-km4-v3"),
        startup_gpio_clock_policy: when(rust_build, "explicit-enable-v1"),
        startup_flash_handshake_policy: None,
        startup_adc_policy: None,
        startup_wifi_policy: when(firstboot, "fw07-dns-v1"),
        startup_transport_policy: when(firstboot, "fw08-transport-v1"),
        startup_optics_policy: when(firstboot && variant.is_none(), "fw13-optics-v2"),
        startup_motion_policy: variant.and_then(Variant::motion_policy).map(Into::into),
        startup_trial_policy: variant.and_then(Variant::trial_policy).map(Into::into),
        variant: variant.map(|v| v.name().into()),
        startup_update_policy: ota_key.as_ref().map(|_| "fw10-recovery-v1".into()),
        ota_public_key: ota_key,
        toolchain_sha256: lock.toolchain.sha256,
        built_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        inputs: expected,
        sdk_overrides: overrides,
        compiled_archives,
        rust_toolchain: compiler.as_ref().map(|c| c.version.clone()),
        rust_compiler: compiler,
        toolchain_files: pins,
        audits,
        bootloader_replacement_supported: false,
        artifacts,
        checks: BuildChecks {
            ota_payload_matches: true,
            ota_checksum_matches: true,
            bringup_banner_present: banner,
            required_core_symbols: required_symbols,
            source_snapshot_unchanged: true,
            sdk_worktree_within_allowlist: true,
            staged_sources_without_diagnostics: true,
        },
        device_compatibility: "Unverified SDK development image; desktop build only".into(),
    };

    let path = build.output.join("manifest.json");
    fs::write(&path, serde_json::to_string_pretty(&manifest)? + "\n")?;
    println!("Build complete; manifest: {}", path.display());
    Ok(())
}

/// Re-run a finished build's audits for its review: the archives come from
/// its manifest, everything else from the build directory and the checkout.
pub fn audit_build(repo: &Repo, build: &Path) -> Result<audit::Linked> {
    let manifest: BuildManifest = petlibre_formats::load(&build.join("manifest.json"))?;
    let objects = manifest
        .compiled_archives
        .into_iter()
        .map(|archive| {
            let core = Core::from_name(&archive.core)
                .ok_or_else(|| Error::Refused(format!("Unknown archive core {}", archive.core)))?;
            Ok((core, build.join(archive.path)))
        })
        .collect::<Result<Vec<_>>>()?;
    audit::linked(repo, build, &objects)
}
