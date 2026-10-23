//! Realtek AmebaD (RTL8721D): the pinned `ameba-rtos-d` SDK and Realtek
//! toolchain, a KM0 and a KM4 core, and the vendor recipes that link and
//! package both into one OTA image.
//!
//! A build cross-compiles one Rust archive per core, runs the vendor
//! recipes in the profile's overlaid SDK worktree ([`sdk`]), checks the
//! packaged image and audits the linked code ([`audit`]). Inputs, overlays,
//! the SDK worktree, the toolchain and the selected rustc are all checked
//! again after compilation, because the vendor recipes take minutes and
//! nothing stops an editor touching the tree meanwhile.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use wellfed_formats::device::AmebadMains;
use wellfed_formats::lock::SdkLock;
use wellfed_formats::manifest::{Artifact, Audits, CompiledArchive};
use regex::Regex;

use self::audit::{Archives, Binutils, park, word};
use self::sdk::{Overrides, check_sdk_worktree, prepare, rust_archive};
use self::toolchain::{toolchain_lock, verify_toolchain};
use super::{Built, Job};
use crate::device::StockPaths;
use crate::environment::Environment;
use crate::{Error, Firmware, Profile, Repo, Result, Stage, digest, relative, require, text};

pub mod audit;
pub(crate) mod credentials;
pub(crate) mod sdk;
pub(crate) mod toolchain;

/// The vendor project inside an SDK worktree.
pub const PROJECT: &str = "project/realtek_amebaD_va0_example/GCC-RELEASE";

const SDK_LOCK: &str = "src/firmware/vendor/sdk.lock.json";
pub(crate) const TOOL_LOCK: &str = "src/firmware/vendor/probe/toolchain.lock.json";
/// The application entry's generated-header inputs: read from ignored
/// private files; only their hashes reach the manifest.
pub(crate) const WIFI_CREDENTIALS: &str = "private/hotspot.json";
pub(crate) const TRANSPORT_CREDENTIALS: &str = "private/fw-mqtt.json";

/// The AmebaD's two cores. The SDK names their projects `lp` and `hp`; the
/// manifest and audits call them KM0 and KM4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Core {
    /// KM0, the low-power Cortex-M23.
    Lp,
    /// KM4, the application Cortex-M33.
    Hp,
}

/// Both cores' Cargo targets.
pub(crate) const TARGETS: [&str; 2] = [Core::Lp.target(), Core::Hp.target()];

impl Core {
    pub const BOTH: [Self; 2] = [Self::Lp, Self::Hp];

    /// The SDK's directory suffix (`project_lp`, `src_hp`).
    pub fn dir(self) -> &'static str {
        match self {
            Self::Lp => "lp",
            Self::Hp => "hp",
        }
    }

    /// The manifest's and audits' name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lp => "KM0",
            Self::Hp => "KM4",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::BOTH.into_iter().find(|core| core.name() == name)
    }

    /// The Cargo target of the core's firmware archive.
    pub(crate) const fn target(self) -> &'static str {
        match self {
            Self::Lp => "thumbv8m.base-none-eabi",
            Self::Hp => "thumbv8m.main-none-eabihf",
        }
    }

    /// The core's feature in its archive crate: `wellfed-sdk-c` for KM0,
    /// the firmware for KM4.
    pub(crate) fn feature(self) -> &'static str {
        match self {
            Self::Lp => "km0",
            Self::Hp => "km4",
        }
    }
}

impl std::fmt::Display for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The pinned SDK checkout every profile's worktree comes from.
pub(crate) fn source(repo: &Repo) -> PathBuf {
    repo.path("private/upstream/ameba-rtos-d")
}

pub(crate) fn toolchains(repo: &Repo) -> PathBuf {
    repo.path("private/toolchains")
}

/// `src/firmware/vendor/sdk.lock.json`.
pub(crate) fn lock(repo: &Repo) -> Result<SdkLock> {
    Ok(wellfed_formats::load(&repo.path(SDK_LOCK))?)
}

/// The pinned toolchain's binutils directory.
pub fn binutils(repo: &Repo) -> Result<PathBuf> {
    Ok(toolchains(repo)
        .join(&lock(repo)?.toolchain.directory)
        .join("bin"))
}

/// A manifest's archives, by core.
pub(crate) fn archives(recorded: &[(String, PathBuf)]) -> Result<Vec<(Core, PathBuf)>> {
    recorded
        .iter()
        .map(|(core, path)| {
            let core = Core::from_name(core)
                .ok_or_else(|| Error::Refused(format!("Unknown archive core {core}")))?;
            Ok((core, path.clone()))
        })
        .collect()
}

/// A device's AmebaD board files: the application entry, the first-boot
/// mains and the BLE peripheral, as checkout paths.
#[derive(Debug, Clone)]
pub(crate) struct Amebad {
    /// The SDK `example_entry.c` replacement; it includes both credential
    /// headers.
    pub(crate) entry: PathBuf,
    pub(crate) mains: [(Core, PathBuf); 2],
    /// Staged beside the entry for a Bluetooth firmware ([`sdk`]).
    pub(crate) bluetooth: Option<PathBuf>,
}

/// C ABI symbols every core archive must define. The core ABI is a C
/// interface for tests and later C consumers, retained in the archive, not
/// the image.
fn archive_symbols(core: Core) -> Vec<&'static str> {
    let mut symbols = match core {
        Core::Hp => vec![
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
/// plus what the SDK crates' build scripts read.
fn cargo_environment(
    environment: &Environment,
    sdk: &Path,
    toolchain: &Path,
    identity: &str,
    ota_key: Option<&str>,
) -> Environment {
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

/// Cross-compile one firmware archive per core and stage it beside the SDK:
/// KM0's from `wellfed-sdk-c`, KM4's from the firmware crate, which links
/// the SDK runtime, with the variant's feature.
fn build_rust(
    repo: &Repo,
    sdk: &Path,
    log: &File,
    environment: &Environment,
    firmware: &Firmware,
    variant: Option<&str>,
) -> Result<Vec<(Core, PathBuf)>> {
    let target_dir = sdk.parent().unwrap_or(sdk).join("rust-target");
    let mut archives = Vec::new();
    for core in Core::BOTH {
        let (package, archive) = match core {
            Core::Lp => (
                "wellfed-sdk-c".to_string(),
                "libwellfed_sdk_c.a".to_string(),
            ),
            Core::Hp => (firmware.package(), firmware.archive()),
        };
        let features = match variant {
            Some(variant) if core == Core::Hp => format!("{},{variant}", core.feature()),
            _ => core.feature().to_string(),
        };
        let status = Command::new("cargo")
            .args(["build", "--locked", "--profile", "firmware", "-p", &package])
            .args(["--target", core.target(), "--features", &features])
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
                .join("firmware")
                .join(archive),
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
        Regex::new(r"/(?:example_entry|wellfed_ble)\.c:.*(?:warning|error):")
            .expect("entry pattern")
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
    if profile.has(Stage::Firstboot) {
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

/// An audit as the manifest records it: the same serialization
/// `wellfed-build audit` prints, which the frozen review compares.
fn recorded(audit: &impl serde::Serialize) -> Result<Option<serde_json::Value>> {
    Ok(Some(serde_json::to_value(audit)?))
}

/// One profile's build in its SDK worktree.
struct Build<'a> {
    job: &'a Job<'a>,
    /// Present for a profile with the stock-layout stage.
    stock: Option<StockPaths>,
    sdk: PathBuf,
    log: PathBuf,
    binutils: Binutils,
}

impl Build<'_> {
    fn image_directory(&self) -> PathBuf {
        self.sdk.join(PROJECT).join("project_hp/asdk/image")
    }

    /// The Rust archives (for a Rust profile), then both cores' vendor
    /// recipes, refusing any compiler diagnostic in the staged sources.
    fn compile(&self) -> Result<Vec<(Core, PathBuf)>> {
        let job = self.job;
        let log = File::create(&self.log)?;
        let logged = |stage| Error::Logged {
            stage,
            log: self.log.clone(),
        };

        let mut archives = Vec::new();
        if let Some(rust) = job.rust {
            println!("Building Rust firmware archives...");
            let environment = cargo_environment(
                &rust.environment,
                &self.sdk,
                job.toolchain,
                job.identity,
                job.ota_key,
            );
            archives = build_rust(
                job.repo,
                &self.sdk,
                &log,
                &environment,
                job.firmware,
                job.variant.map(|v| v.name()),
            )
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

        let diagnostics = staged_diagnostics(&self.log, job.profile);
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
                path: relative(&path, &self.job.output)?,
                bytes,
                sha256: digest(&path)?,
            });
        }
        Ok(artifacts)
    }

    /// The OTA container holds the combined image, which runs the requested
    /// application and embeds the identity. Returns the OTA and whether the
    /// Rust banner is present.
    fn check_image(&self) -> Result<(Vec<u8>, bool)> {
        let combined = fs::read(self.image_directory().join("km0_km4_image2.bin"))?;
        let ota = fs::read(self.image_directory().join("OTA_All.bin"))?;
        check_container(&ota, &combined)?;

        let rust = self.job.profile.has(Stage::Rust);
        let banner = contains(&combined, b"Wellfed local firmware: Rust bringup");
        require(
            banner == rust,
            "Linked image does not match the requested application profile",
        )?;
        require(
            !rust
                || contains(
                    &combined,
                    format!("PLFW1 start build={} ", self.job.identity).as_bytes(),
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
            "wellfed_app_start",
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
                path: relative(archive, &self.job.output)?,
                sha256: digest(archive)?,
                symbols: symbols.iter().map(|s| s.to_string()).collect(),
                hardware_activated: false,
            });
        }
        Ok((required.map(String::from).to_vec(), compiled))
    }

    /// Nothing the build read changed while it compiled. Returns the
    /// toolchain's file pins.
    fn recheck(&self, overrides: &Overrides, lock: &SdkLock) -> Result<BTreeMap<String, String>> {
        let job = self.job;
        (job.recheck)()?;
        for (path, checksum) in overrides {
            require(
                &digest(&self.sdk.join(path))? == checksum,
                "Generated SDK layout override changed during compilation",
            )?;
        }
        check_sdk_worktree(&self.sdk, job.profile)?;

        let pins = toolchain_lock(job.repo, lock)?;
        verify_toolchain(job.toolchain, &pins)?;
        Ok(pins)
    }

    /// The profile's linked-image audits, as the manifest records them.
    fn audits(&self, ota: &[u8], archives: &Archives) -> Result<Audits> {
        let job = self.job;
        let (sdk, binutils) = (&self.sdk, &self.binutils);

        let mut audits = Audits::default();
        if let Some(stock) = &self.stock {
            let image = fs::read(&stock.image).unwrap_or_default();
            let layout = audit::layout::audit(
                &self.image_directory().join("OTA_All.bin"),
                &stock.image,
                &stock.layout,
            )
            .map_err(|e| Error::Layout(Box::new(e)))?;
            audits.layout_audit = recorded(&layout)?;
            audits.km0_pinmap_audit = recorded(&audit::pinmap::audit_linked_table(
                ota,
                audit::pmap_address(sdk, binutils)?,
                &image,
            )?)?;
        }
        if job.profile.has(Stage::Rust) {
            audits.ram_park_audit = recorded(&park::audit(sdk, binutils, archives)?)?;
            audits.sdk_hook_audit = recorded(&audit::hooks::audit(sdk, binutils, archives)?)?;
        }
        if job.profile.has(Stage::Firstboot) {
            audits.firstboot_startup_audit = recorded(&audit::mains::audit(sdk, binutils)?)?;
            audits.wifi_link_audit = recorded(&audit::writes::audit(sdk, binutils)?)?;
            audits.stack_audit = recorded(&audit::stack::audit(
                &self.image_directory().join("target_img2.axf"),
                &binutils.objdump(),
                &audit::stack::task_sources(job.repo, job.firmware)?,
            )?)?;
        }
        Ok(audits)
    }
}

impl Amebad {
    pub(crate) fn new(
        repo: &Repo,
        entry: &str,
        mains: &AmebadMains,
        bluetooth: Option<&str>,
    ) -> Self {
        Self {
            entry: repo.path(entry),
            mains: [
                (Core::Lp, repo.path(&mains.km0)),
                (Core::Hp, repo.path(&mains.km4)),
            ],
            bluetooth: bluetooth.map(|path| repo.path(path)),
        }
    }

    /// The BLE source a Rust profile of `firmware` stages, if it is a
    /// Bluetooth firmware; refused when the board has none.
    pub(crate) fn bluetooth_source(&self, firmware: &Firmware) -> Result<Option<&Path>> {
        if !firmware.bluetooth() {
            return Ok(None);
        }
        let Some(source) = &self.bluetooth else {
            return Err(Error::Refused(format!(
                "Firmware {} needs Bluetooth, which this device does not declare",
                firmware.name()
            )));
        };
        Ok(Some(source))
    }

    /// The pins, and the board files a profile stages.
    pub(crate) fn inputs(
        &self,
        repo: &Repo,
        profile: Profile,
        firmware: &Firmware,
    ) -> Result<Vec<PathBuf>> {
        let mut paths = vec![repo.path(SDK_LOCK), repo.path(TOOL_LOCK)];
        if profile.has(Stage::Rust) {
            // The entry includes both generated credential headers.
            paths.push(self.entry.clone());
            paths.extend([WIFI_CREDENTIALS, TRANSPORT_CREDENTIALS].map(|p| repo.path(p)));
            paths.extend(self.bluetooth_source(firmware)?.map(Path::to_path_buf));
        }
        if profile.has(Stage::Firstboot) {
            paths.extend(self.mains.iter().map(|(_, path)| path.clone()));
        }
        Ok(paths)
    }

    pub(crate) fn build(&self, job: &Job) -> Result<Built> {
        let lock = lock(job.repo)?;
        let stock = if job.profile.has(Stage::Stock) {
            let stock = job.device.stock(job.repo)?;
            audit::layout::audit(&stock.image, &stock.image, &stock.layout)
                .map_err(|e| Error::Layout(Box::new(e)))?;
            Some(stock)
        } else {
            None
        };

        let (sdk, overrides) = prepare(job, self, stock.as_ref(), &lock)?;
        let build = Build {
            job,
            stock,
            log: job.log.clone(),
            sdk,
            binutils: Binutils {
                dir: job.toolchain.join("bin"),
            },
        };

        let archives = build.compile()?;
        let artifacts = build.artifacts()?;
        let (ota, banner) = build.check_image()?;
        let (required_symbols, compiled_archives) = if job.profile.has(Stage::Rust) {
            build.check_symbols(&archives)?
        } else {
            Default::default()
        };
        let toolchain_files = build.recheck(&overrides, &lock)?;
        let audits = build.audits(&ota, &archives)?;

        Ok(Built {
            sdk_revision: lock.sdk.revision,
            toolchain_sha256: lock.toolchain.sha256,
            toolchain_files,
            sdk_overrides: overrides,
            artifacts,
            compiled_archives,
            banner,
            required_symbols,
            audits,
        })
    }
}
