#!/usr/bin/env python3
"""Build AmebaD experiments locally, without connecting to the feeder."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

import pinmap
import rampark
import sdkhooks
import firstboot


FIRMWARE = Path(__file__).resolve().parents[1]
ROOT = FIRMWARE.parent
PRIVATE = ROOT / "private"
SOURCE = PRIVATE / "upstream/ameba-rtos-d"
TOOLCHAINS = PRIVATE / "toolchains"
PROJECT = Path("project/realtek_amebaD_va0_example/GCC-RELEASE")
LOCK = json.loads((FIRMWARE / "sdk.lock.json").read_text())
TOOL_LOCK = FIRMWARE / "probe/toolchain.lock.json"
LAYOUT = FIRMWARE / "plaf103-layout.json"
AUDITOR = ROOT / "tools/audit-ota-layout.py"
STOCK = PRIVATE / "firmware/PLAF103_OTA_010115.bin"
RUST = FIRMWARE / "rust"
ENTRY = FIRMWARE / "ports/amebad/example_entry.c"
MODEL_PROFILES = ("plaf103", "plaf103-firstboot")
# Cargo target and features for each SDK core project.
RUST_CORES = {"lp": ("thumbv8m.base-none-eabi", "km0"), "hp": ("thumbv8m.main-none-eabihf", "km4")}
# C ABI symbols every KM4 archive must define; the core ABI is a C interface
# for tests and later C consumers, retained in the archive, not the image.
ARCHIVE_SYMBOLS = {
    "hp": ["petlibro_app_start", "petlibro_diagnostic_run", "petlibro_feed_controller_update",
           "petlibro_sensor_inputs_update", "petlibro_adc_scale_16", "petlibro_motor_gate_request",
           "petlibro_feedback_events_process", "petlibro_tick_clock_read", "petlibro_flash_coord_init",
           "petlibro_flash_coord_request", "petlibro_flash_coord_parked", "petlibro_flash_coord_released",
           "petlibro_flash_coord_outputs_blocked", "petlibro_feed_journal_open",
           "petlibro_feed_journal_create", "petlibro_feed_journal_read", "petlibro_feed_journal_storage"]
        + sorted(rampark.SYMBOLS) + ["FLASH_Write_Lock", "FLASH_Write_Unlock", "FLASH_Write_IPC_Int"],
    "lp": sorted(rampark.SYMBOLS) + ["FLASH_Write_Lock", "FLASH_Write_Unlock", "FLASH_Write_IPC_Int"],
}


def run(*args, cwd=None, **kwargs):
    return subprocess.run(args, cwd=cwd, check=True, **kwargs)


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def revision(path):
    return run("git", "rev-parse", "HEAD", cwd=path,
               capture_output=True, text=True).stdout.strip()


# Exact SDK worktree changes the pinned vendor recipes make themselves, observed
# in every profile's worktree: `check_execute_permission` sets the executable
# bit on these helpers (content must stay identical to the pinned revision) ...
VENDOR_MODE_ONLY = frozenset(
    str(PROJECT / f"project_{core}/asdk/gnu_utility" / name)
    for core in ("hp", "lp")
    for name in ("checksum", "image_tool/imagetool.sh", "pad.sh", "prepend_header.sh",
                 "prepend_ota_header.sh"))
# ... the KM4 recipe re-archives this tracked library on every build ...
VENDOR_REBUILT = frozenset({str(PROJECT / "project_hp/asdk/lib/application/lib_user.a")})
# ... and these untracked paths are the wrapper's toolchain links (checked
# separately) and two KM0 loader outputs copied by the vendor recipe.
VENDOR_UNTRACKED = frozenset({
    str(PROJECT / "project_hp/toolchain/linux"), str(PROJECT / "project_lp/toolchain/linux"),
    str(PROJECT / "project_lp/asdk/gnu_utility/gnu_script/acut_loader/flash_loader_ram_1.bin"),
    str(PROJECT / "project_lp/asdk/gnu_utility/gnu_script/acut_loader/target_FPGA.axf")})
RUST_ENVIRONMENT = frozenset({
    "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC", "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN", "RUSTC_BOOTSTRAP",
    "CARGO_INCREMENTAL", "CARGO_BUILD_INCREMENTAL"})
RUST_ENVIRONMENT_PREFIXES = ("CARGO_PROFILE_", "CARGO_UNSTABLE_")


def managed_sdk_paths(profile):
    """SDK paths this wrapper regenerates from the pinned revision for a profile."""
    paths = set()
    if profile in MODEL_PROFILES:
        paths |= {"component/soc/realtek/amebad/fwlib/include/rtl8721d_ota.h",
                  "component/soc/realtek/amebad/fwlib/usrcfg/rtl8721dlp_pinmapcfg.c"}
    if profile != "baseline":
        paths |= {"component/common/example/example_entry.c", str(sdkhooks.SOURCE),
                  str(PROJECT / "project_lp/asdk/Makefile"), str(PROJECT / "project_hp/asdk/Makefile"),
                  str(PROJECT / "project_lp/asdk/ld/rlx8721d_img2.ld"),
                  str(PROJECT / "project_hp/asdk/ld/rlx8721d_img2_is.ld"),
                  # Restored from the pinned revision before every application build.
                  str(PROJECT / "project_hp/asdk/make/utilities_example/Makefile"),
                  "project/realtek_amebaD_va0_example/src/src_lp/Makefile"}
    if profile == "plaf103-firstboot":
        paths |= {f"project/realtek_amebaD_va0_example/src/src_{core}/main.c" for core in ("lp", "hp")}
    return paths


def unexpected_sdk_changes(sdk, profile):
    """Return worktree changes outside the wrapper's overrides and known vendor outputs.

    Ignored files are governed by the pinned SDK's own ignore rules and are not listed.
    """
    status = run("git", "status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames",
                 cwd=sdk, capture_output=True).stdout.decode("utf-8", "surrogateescape")
    managed = managed_sdk_paths(profile)
    unexpected = []
    for entry in filter(None, status.split("\0")):
        code, path = entry[:2], entry[3:]
        if code == "??":
            # Flat C inputs staged by earlier wrappers are retired before compilation.
            retired = profile != "baseline" and re.fullmatch(r"component/common/example/petlibro/[^/]+\.[ch]", path)
            allowed = path in VENDOR_UNTRACKED or bool(retired)
        elif code == " M":
            allowed = path in managed or path in VENDOR_REBUILT or (
                path in VENDOR_MODE_ONLY and os.access(sdk / path, os.X_OK) and
                run("git", "hash-object", "--", path, cwd=sdk, capture_output=True, text=True).stdout.strip()
                == run("git", "rev-parse", "HEAD:" + path, cwd=sdk, capture_output=True, text=True).stdout.strip())
        else:
            allowed = False  # Staged, deleted, type-changed or conflicted entries.
        if not allowed:
            unexpected.append(f"{code.strip() or code} {path}")
    return sorted(unexpected)


def check_sdk_worktree(sdk, profile):
    unexpected = unexpected_sdk_changes(sdk, profile)
    if unexpected:
        shown = ", ".join(unexpected[:20]) + (f" (+{len(unexpected) - 20} more)" if len(unexpected) > 20 else "")
        raise RuntimeError("SDK worktree has changes outside this profile's overrides; left unchanged. "
                           f"Review, then restore or recreate {sdk}: {shown}")


def cargo_config_files(environment):
    """Cargo configuration files that apply to a build run from ROOT."""
    home = Path(environment.get("CARGO_HOME") or Path.home() / ".cargo")
    directories = [ROOT, *ROOT.parents]
    candidates = [d / ".cargo" / name for d in directories for name in ("config.toml", "config")]
    candidates += [home / "config.toml", home / "config"]
    return [p for p in dict.fromkeys(candidates) if p.is_file()]


def cargo_config_overrides(text):
    """Return dotted keys that can change firmware codegen or the selected compiler.

    A small line-oriented reader: TOML table headers and `key = value` lines.
    Inline tables for `build`, `profile`, `target` or `unstable` are refused whole.
    Host-only `[target.<host triple>]` settings (such as a host linker) are allowed.
    """
    table = []
    found = []
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        header = re.fullmatch(r"\[\[?\s*([^\]]+?)\s*\]\]?", line)
        if header:
            table = keys(header[1])
            continue
        match = re.match(r"([^=]+?)\s*=", line)
        if not match:
            continue  # Continuation of a multi-line value.
        key = table + keys(match[1])
        if forbidden_cargo_key(key, line[match.end():].lstrip().startswith("{")):
            found.append(".".join(key))
    return found


def keys(dotted):
    return [part.strip().strip("\"'") for part in re.findall(r"\"[^\"]*\"|'[^']*'|[^.]+", dotted)]


def forbidden_cargo_key(key, inline):
    head = key[0] if key else ""
    if head in ("profile", "unstable"):
        return True
    if head == "build":
        return len(key) == 1 or key[1] in ("rustflags", "rustc", "rustc-wrapper",
                                           "rustc-workspace-wrapper", "incremental")
    if head == "target" and len(key) >= 2:
        selector = key[1]
        firmware = selector.startswith("cfg(") or selector in {t for t, _ in RUST_CORES.values()}
        return firmware and (len(key) == 2 and inline or len(key) > 2 and key[2] == "rustflags")
    if head == "target":
        return inline
    return False


def rust_environment():
    """The cargo environment, refusing output-affecting overrides rather than scrubbing them.

    Refusing keeps an explicit user choice visible; silently dropping it would
    make the build differ from what the caller asked for.
    """
    environment = dict(os.environ)
    names = sorted(name for name in environment if name in RUST_ENVIRONMENT or
                   name.startswith(RUST_ENVIRONMENT_PREFIXES) or
                   re.fullmatch(r"CARGO_TARGET_THUMBV8M_[A-Z_]+_RUSTFLAGS", name))
    if names:
        raise RuntimeError("Unset compiler/profile overrides before a firmware build: " + ", ".join(names))
    for path in cargo_config_files(environment):
        found = cargo_config_overrides(path.read_text(errors="replace"))
        if found:
            raise RuntimeError(f"Cargo configuration overrides firmware compilation in {path}: " + ", ".join(found))
    return environment


def rust_compiler(environment):
    """Describe the rustc that cargo resolves from ROOT, and require the pinned channel."""
    channel = re.search(r'(?m)^channel\s*=\s*"([^"]+)"', (ROOT / "rust-toolchain.toml").read_text())
    verbose = run("rustc", "-vV", cwd=ROOT, env=environment, capture_output=True, text=True).stdout
    release = re.search(r"(?m)^release:\s*(\S+)$", verbose)
    if not channel or not release or release[1] != channel[1]:
        raise RuntimeError("rustc selected from the repository does not match rust-toolchain.toml")
    path = None
    if shutil.which("rustup", path=environment.get("PATH")):
        path = run("rustup", "which", "rustc", cwd=ROOT, env=environment,
                   capture_output=True, text=True).stdout.strip()
    return {"version": verbose.splitlines()[0], "verbose": verbose.strip().splitlines(), "path": path}


def toolchain_lock():
    """Per-file hashes of the executables the SDK build and audits invoke."""
    pinned = json.loads(TOOL_LOCK.read_text())
    if pinned["archive_sha256"] != LOCK["toolchain"]["sha256"] or not pinned["files"]:
        raise RuntimeError("Toolchain file pins do not describe the pinned SDK archive")
    return pinned["files"]


def verify_toolchain(toolchain):
    """Fail closed unless every pinned compiler/binutils file matches, on every run."""
    base = toolchain.resolve()
    for name, expected in toolchain_lock().items():
        path = (toolchain / name).resolve()
        if not path.is_relative_to(base) or not path.is_file():
            raise RuntimeError(f"Pinned toolchain file is missing or escapes its directory: {name}")
        if digest(path) != expected:
            raise RuntimeError(f"Pinned toolchain file changed: {name}; reinstall after review")


def extract_toolchain(archive, destination, directory):
    """Extract into a private temporary directory, verify, then rename into place.

    An interrupted extraction leaves only an ignored `.extract-*` directory and
    never a partial tree that later runs would accept.
    """
    # Python's data filter rejects archive paths escaping the destination.
    if not hasattr(tarfile, "data_filter"):
        raise RuntimeError("Use a patched Python with tarfile's data filter (3.10.12+)")
    top = Path(directory).parts[0]
    if (destination / top).exists() or (destination / top).is_symlink():
        raise RuntimeError(f"Partial or unexpected toolchain tree; review and remove it: {destination / top}")
    staging = Path(tempfile.mkdtemp(prefix=".extract-", dir=destination))
    try:
        with tarfile.open(archive) as bundle:
            names = bundle.getnames()
            if not names or any(Path(name).parts[:1] != (top,) for name in names):
                raise RuntimeError("Toolchain archive has entries outside its pinned directory")
            bundle.extractall(staging, filter="data")
        verify_toolchain(staging / directory)
        os.rename(staging / top, destination / top)
    finally:
        # Only this run's private staging directory is removed.
        shutil.rmtree(staging)


def bootstrap():
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise RuntimeError("This initial toolchain setup supports Linux x86_64 only")
    for tool in ("git", "make", "bash", "tar"):
        if not shutil.which(tool):
            raise RuntimeError(f"Missing host tool: {tool}")
    if not SOURCE.exists():
        SOURCE.parent.mkdir(parents=True, exist_ok=True)
        run("git", "init", str(SOURCE))
        run("git", "remote", "add", "origin", LOCK["sdk"]["url"], cwd=SOURCE)
        run("git", "fetch", "--depth", "1", "origin",
            LOCK["sdk"]["revision"], cwd=SOURCE)
        run("git", "checkout", "--detach", "FETCH_HEAD", cwd=SOURCE)
    if revision(SOURCE) != LOCK["sdk"]["revision"]:
        raise RuntimeError("SDK checkout does not match sdk.lock.json; left unchanged")

    TOOLCHAINS.mkdir(parents=True, exist_ok=True)
    archive = TOOLCHAINS / LOCK["toolchain"]["url"].rsplit("/", 1)[1]
    if not archive.exists():
        partial = archive.with_suffix(archive.suffix + ".partial")
        print("Downloading pinned Realtek toolchain...", flush=True)
        with urllib.request.urlopen(LOCK["toolchain"]["url"], timeout=60) as response:
            with partial.open("wb") as output:
                shutil.copyfileobj(response, output)
        if digest(partial) != LOCK["toolchain"]["sha256"]:
            raise RuntimeError(f"Toolchain checksum mismatch: {partial}")
        partial.rename(archive)
    if digest(archive) != LOCK["toolchain"]["sha256"]:
        raise RuntimeError(f"Toolchain checksum mismatch: {archive}")
    toolchain = TOOLCHAINS / LOCK["toolchain"]["directory"]
    if not toolchain.exists():
        extract_toolchain(archive, TOOLCHAINS, LOCK["toolchain"]["directory"])
    verify_toolchain(toolchain)
    compiler = toolchain / "bin/arm-none-eabi-gcc"
    try:
        version = run(str(compiler), "--version", capture_output=True, text=True)
    except OSError as error:
        raise RuntimeError(
            "Cannot execute Realtek's i686 compiler; check the host's 32-bit "
            "Linux loader and runtime libraries"
        ) from error
    print(version.stdout.splitlines()[0], flush=True)
    return toolchain


def rust_sources():
    """Tracked plus untracked, non-ignored files under firmware/rust.

    Ignored files (editor backups, caches, build output) do not change the
    identity; `target` is excluded only as a directory below firmware/rust, so
    the checkout's own location never matters.
    """
    try:
        listing = run("git", "ls-files", "-z", "--cached", "--others", "--exclude-standard",
                      "--", str(RUST.relative_to(ROOT)), cwd=ROOT, capture_output=True).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError("Source identity requires a Git checkout of this repository") from error
    paths = set()
    for name in listing.decode("utf-8", "surrogateescape").split("\0"):
        if not name:
            continue
        path = ROOT / name
        # Tracked files deleted from the working tree are simply not inputs.
        if "target" not in path.relative_to(RUST).parts and path.is_file():
            paths.add(path)
    if not paths:
        raise RuntimeError("No Rust firmware sources found for the source identity")
    return sorted(paths)


def source_inputs(profile):
    paths = [FIRMWARE / "sdk.lock.json", TOOL_LOCK, Path(__file__).resolve()]
    if profile != "baseline":
        paths += [FIRMWARE / "tools/rampark.py", FIRMWARE / "tools/sdkhooks.py", AUDITOR,
                  ROOT / "tools/audit-sdk-flash-users.py"]
        paths += [ENTRY, ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "rust-toolchain.toml"]
        paths += rust_sources()
    if profile in MODEL_PROFILES:
        paths += [LAYOUT, AUDITOR, FIRMWARE / "tools/pinmap.py"]
    if profile == "plaf103-firstboot":
        paths += [FIRMWARE / "tools/firstboot.py", *firstboot.sources(FIRMWARE).values()]
    return list(dict.fromkeys(paths))


def source_identity(profile, inputs):
    """Bind the build identity to a profile and root-relative input hashes."""
    snapshot = {"profile": profile,
                "inputs": {str(p.relative_to(ROOT)): checksum for p, checksum in inputs.items()}}
    return hashlib.sha256(json.dumps(snapshot, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def audit_image(image):
    try:
        result = run(sys.executable, str(AUDITOR), str(image), "--stock-reference", str(STOCK),
                     "--layout", str(LAYOUT), capture_output=True, text=True)
    except subprocess.CalledProcessError as error:
        raise RuntimeError("Offline layout audit failed: " + error.stderr.strip()) from None
    return json.loads(result.stdout)


def rust_archive(sdk, core):
    return sdk.parent / "rust-lib" / f"libpetlibro_{RUST_CORES[core][1]}.a"


def build_rust(sdk, toolchain, stream, identity, base_environment):
    """Cross-compile one firmware archive per core and stage it beside the SDK."""
    target_dir = sdk.parent / "rust-target"
    environment = dict(base_environment, PETLIBRO_BUILD_ID=identity, PETLIBRO_SDK_DIR=str(sdk),
                       PETLIBRO_NEWLIB_INCLUDE=str(toolchain / "arm-none-eabi/include"))
    archives = {}
    for core, (triple, feature) in RUST_CORES.items():
        run("cargo", "build", "--locked", "--profile", "firmware", "-p", "petlibro-ffi",
            "--target", triple, "--features", feature, "--target-dir", str(target_dir),
            cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT)
        built = target_dir / triple / "firmware/libpetlibro_ffi.a"
        staged = rust_archive(sdk, core)
        staged.parent.mkdir(exist_ok=True)
        shutil.copyfile(built, staged)
        archives[core] = staged
    return archives


def archive_symbols(toolchain, archive, required):
    symbols = run(str(toolchain / "bin/arm-none-eabi-nm"), "--defined-only", str(archive),
                  capture_output=True, text=True).stdout
    names = {line.split()[-1] for line in symbols.splitlines() if len(line.split()) == 3}
    missing = sorted(set(required) - names)
    if missing:
        raise RuntimeError("Rust archive is missing expected symbols: " + ", ".join(missing))


def prepare(profile, toolchain, expected):
    sdk = PRIVATE / "firmware-build" / profile / "sdk"
    if not sdk.exists():
        sdk.parent.mkdir(parents=True, exist_ok=True)
        run("git", "worktree", "add", "--detach", str(sdk),
            LOCK["sdk"]["revision"], cwd=SOURCE)
    if revision(sdk) != LOCK["sdk"]["revision"]:
        raise RuntimeError("Build worktree has an unexpected SDK revision")
    check_sdk_worktree(sdk, profile)
    # Both cores use the same verified toolchain, rather than downloading copies.
    for core in ("lp", "hp"):
        link = sdk / PROJECT / f"project_{core}/toolchain/linux"
        target = TOOLCHAINS
        if link.is_symlink() and link.resolve() == target:
            continue
        if link.exists() or link.is_symlink():
            raise RuntimeError(f"Unexpected toolchain path; left unchanged: {link}")
        link.symlink_to(target, target_is_directory=True)
    overrides = {}
    if profile in MODEL_PROFILES:
        layout = json.loads(LAYOUT.read_text())
        relative = Path("component/soc/realtek/amebad/fwlib/include/rtl8721d_ota.h")
        original = run("git", "show", "HEAD:" + str(relative), cwd=sdk,
                       capture_output=True, text=True).stdout
        addresses = [int(layout["flash_mapping_base"], 16) + int(x, 16)
                     for x in layout["ota_bank_offsets"]]
        for index, address in enumerate(addresses, 1):
            pattern = rf"(#define\s+LS_IMG2_OTA{index}_ADDR\s+)0x[0-9A-Fa-f]+"
            original, count = re.subn(pattern, lambda m: m[1] + f"0x{address:08X}", original)
            if count != 1:
                raise RuntimeError("Pinned SDK OTA address definition changed")
        (sdk / relative).write_text(original)
        overrides[str(relative)] = digest(sdk / relative)
        relative = Path("component/soc/realtek/amebad/fwlib/usrcfg/rtl8721dlp_pinmapcfg.c")
        original = run("git", "show", "HEAD:" + str(relative), cwd=sdk,
                       capture_output=True, text=True).stdout
        (sdk / relative).write_text(pinmap.overlay(original, STOCK.read_bytes()))
        overrides[str(relative)] = digest(sdk / relative)
    if profile != "baseline":
        example = sdk / "component/common/example"
        application = example / "petlibro"
        previous = list(application.rglob("*.[ch]")) if application.exists() else []
        wanted = set()  # The application is the Rust archive; no C is staged here.
        # Preserve retired generated inputs outside the SDK's basename-based
        # vpath search. Otherwise old flat sources can shadow their replacements.
        retired = sdk.parent / "retired-app-inputs"
        for path in previous:
            if path not in wanted:
                retired.mkdir(exist_ok=True)
                archive = retired / (digest(path) + "-" + path.name)
                if not archive.exists():
                    shutil.copyfile(path, archive)
                path.unlink()
        # The vendor makefiles name objects by basename and include old .d files.
        # Rebuild only our objects, including retired ones, to avoid stale links.
        names = {p.stem for p in previous if p.suffix == ".c"}
        names |= {"plaf103_flash_park", "plaf103_flash_hooks"}
        object_root = sdk / PROJECT / "project_hp/asdk"
        for name in names:
            for suffix in (".o", ".d", ".i", ".s", ".su"):
                (object_root / "make/utilities_example" / (name + suffix)).unlink(missing_ok=True)
            (object_root / "build/ram" / (name + ".o")).unlink(missing_ok=True)
        destination = example / "example_entry.c"
        shutil.copyfile(ENTRY, destination)
        if digest(destination) != expected[ENTRY]:
            raise RuntimeError(f"Source changed while staging: {ENTRY}")
        # Earlier C builds modified these SDK makefiles; restore the pinned copies.
        for relative in (PROJECT / "project_hp/asdk/make/utilities_example/Makefile",
                         Path("project/realtek_amebaD_va0_example/src/src_lp/Makefile")):
            run("git", "checkout", "HEAD", "--", str(relative), cwd=sdk)
        # Both cores link the Rust archive: the KM4 application plus, on both,
        # the RAM substrate and SDK interposer. The diagnostic does not install
        # a transport provider; legacy startup remains selected.
        for core in ("lp", "hp"):
            relative = PROJECT / f"project_{core}/asdk/Makefile"
            original = run("git", "show", "HEAD:" + str(relative), cwd=sdk,
                           capture_output=True, text=True).stdout
            anchor = "target_img2.axf $(RAM_OBJS_LIST)"
            if original.count(anchor) != 1:
                raise RuntimeError("Pinned image2 link rule changed")
            (sdk / relative).write_text(original.replace(anchor, anchor + " " + str(rust_archive(sdk, core))))
            overrides[str(relative)] = digest(sdk / relative)
        for directory in (PROJECT / "project_lp/asdk/make/project/src",
                          PROJECT / "project_lp/asdk/build/ram",
                          Path("project/realtek_amebaD_va0_example/src/src_lp")):
            for name in ("plaf103_flash_park", "plaf103_flash_hooks"):
                for suffix in (".o", ".d", ".i", ".s", ".su"):
                    (sdk / directory / (name + suffix)).unlink(missing_ok=True)
        for core, linker in (("lp", "rlx8721d_img2.ld"), ("hp", "rlx8721d_img2_is.ld")):
            relative = PROJECT / f"project_{core}/asdk/ld" / linker
            original = run("git", "show", "HEAD:" + str(relative), cwd=sdk,
                           capture_output=True, text=True).stdout
            anchor = "*(.image2.ram.text*)"
            if original.count(anchor) != 1:
                raise RuntimeError("Pinned RAM linker section changed")
            (sdk / relative).write_text(original.replace(anchor,
                "KEEP(*(.image2.ram.text.petlibro_park))\n\t\t" + anchor))
            overrides[str(relative)] = digest(sdk / relative)
        original = run("git", "show", "HEAD:" + str(sdkhooks.SOURCE), cwd=sdk,
                       capture_output=True, text=True).stdout
        (sdk / sdkhooks.SOURCE).write_text(sdkhooks.overlay(original))
        overrides[str(sdkhooks.SOURCE)] = digest(sdk / sdkhooks.SOURCE)
        # SDK dependencies can retain the old public definitions otherwise.
        for core in ("lp", "hp"):
            for path in (sdk / PROJECT / f"project_{core}/asdk").rglob("rtl8721d_flash_ram.*"):
                if path.suffix in (".o", ".d", ".i", ".s", ".su"):
                    path.unlink()
    if profile == "plaf103-firstboot":
        for core, source in firstboot.sources(FIRMWARE).items():
            relative = Path("project/realtek_amebaD_va0_example/src") / f"src_{core}" / "main.c"
            shutil.copyfile(source, sdk / relative)
            if digest(sdk / relative) != expected[source]:
                raise RuntimeError("First-boot main changed while staging")
            overrides[str(relative)] = digest(sdk / relative)
    return sdk, overrides


def build(profile, toolchain):
    output = PRIVATE / "firmware-build" / profile
    output.mkdir(parents=True, exist_ok=True)
    (output / "manifest.json").unlink(missing_ok=True)
    rust_env = rust_environment() if profile != "baseline" else None
    compiler = rust_compiler(rust_env) if profile != "baseline" else None
    expected = {p: digest(p) for p in source_inputs(profile)}
    identity = source_identity(profile, expected)
    if profile in MODEL_PROFILES:
        audit_image(STOCK)
    sdk, overrides = prepare(profile, toolchain, expected)
    output = sdk.parent
    log = output / "build.log"
    # The vendor recipes aren't safe for parallel make; keep their default order.
    environment = dict(os.environ)
    environment.pop("MAKEFLAGS", None)
    environment.pop("MFLAGS", None)
    print(f"Building {profile}; log: {log}", flush=True)
    with log.open("w") as stream:
        archives = {}
        if profile != "baseline":
            print("Building Rust firmware archives...", flush=True)
            try:
                archives = build_rust(sdk, toolchain, stream, identity, rust_env)
            except subprocess.CalledProcessError:
                raise RuntimeError(f"Rust firmware build failed; see {log}") from None
        for core in ("lp", "hp"):
            print(f"Building KM{'0' if core == 'lp' else '4'}...", flush=True)
            try:
                run("make", "all", cwd=sdk / PROJECT / f"project_{core}",
                    env=environment, stdout=stream, stderr=subprocess.STDOUT)
            except subprocess.CalledProcessError:
                stream.flush()
                lines = log.read_text(errors="replace").splitlines()
                errors = [line for line in lines if re.search(
                    r"error:|undefined reference|No rule to make target|Error [1-9]", line)]
                # SDK command lines can span tens of thousands of characters.
                # Keep the full private log, but surface bounded diagnostics.
                for line in (errors or lines[-12:])[-20:]:
                    print(line if len(line) <= 600 else line[:597] + "...")
                raise RuntimeError(f"SDK build failed; see {log}") from None
    staged_diagnostics = [line for line in log.read_text(errors="replace").splitlines()
                          if re.search(r"(?:/petlibro/.*\.[ch]|example_entry\.c):.*(?:warning|error):", line)]
    if profile == "plaf103-firstboot":
        staged_diagnostics += [line for line in log.read_text(errors="replace").splitlines()
                               if re.search(r"/src_(?:lp|hp)/main\.c:.*(?:warning|error):", line)]
    if staged_diagnostics:
        for line in staged_diagnostics[:20]:
            print(line[:600])
        raise RuntimeError(f"Staged firmware source has compiler diagnostics; see {log}")
    artifacts = []
    for core, name in (("lp", "km0_image2_all.bin"),
                       ("hp", "km4_image2_all.bin"),
                       ("hp", "km0_km4_image2.bin"), ("hp", "OTA_All.bin")):
        path = sdk / PROJECT / f"project_{core}/asdk/image" / name
        if not path.is_file() or path.stat().st_size == 0:
            raise RuntimeError(f"Missing build artifact: {path}")
        artifacts.append({"path": str(path.relative_to(output)),
                          "bytes": path.stat().st_size, "sha256": digest(path)})
    image_directory = sdk / PROJECT / "project_hp/asdk/image"
    combined = (image_directory / "km0_km4_image2.bin").read_bytes()
    ota = (image_directory / "OTA_All.bin").read_bytes()
    if len(ota) < 32:
        raise RuntimeError("OTA container is shorter than its header")
    version, count, magic, entry_size, checksum, size, offset, address = (
        struct.unpack_from("<8I", ota)
    )
    if (version, count, magic, entry_size, offset, address) != (
        0xFFFFFFFF, 1, int.from_bytes(b"OTA1", "little"), 24, 32, 0xFFFFFFFF
    ) or size != len(combined) or ota[32:] != combined:
        raise RuntimeError("OTA container does not match the combined image")
    if sum(combined) & 0xFFFFFFFF != checksum:
        raise RuntimeError("OTA container checksum does not match")
    banner_present = b"Petlibro local firmware: Rust bringup" in combined
    if banner_present != (profile != "baseline"):
        raise RuntimeError("Linked image does not match the requested application profile")
    if profile != "baseline" and f"PLFW1 start build={identity} ".encode() not in combined:
        raise RuntimeError("Linked image is missing the source-bound startup identity")
    required_symbols = []
    compiled_archives = []
    if profile != "baseline":
        # The linked KM4 image must contain the Rust application entry and the
        # SDK interposer; park routines are checked by the RAM park audit.
        required_symbols = ["petlibro_app_start", "FLASH_Write_Lock", "FLASH_Write_Unlock",
                            "FLASH_Write_IPC_Int"]
        symbols = run(str(toolchain / "bin/arm-none-eabi-nm"), "--defined-only",
                      str(image_directory / "target_img2.axf"), capture_output=True, text=True)
        names = {line.split()[-1] for line in symbols.stdout.splitlines() if line.split()}
        if not set(required_symbols) <= names:
            raise RuntimeError("Linked image is missing required application/core symbols")
        for core, archive in archives.items():
            archive_symbols(toolchain, archive, ARCHIVE_SYMBOLS[core])
            compiled_archives.append({"core": "KM0" if core == "lp" else "KM4",
                                      "target": RUST_CORES[core][0],
                                      "path": str(archive.relative_to(output)), "sha256": digest(archive),
                                      "symbols": ARCHIVE_SYMBOLS[core], "hardware_activated": False})
    if {p: digest(p) for p in source_inputs(profile)} != expected:
        raise RuntimeError("Sources changed during the build; rerun before trusting these outputs")
    for relative, checksum in overrides.items():
        if digest(sdk / relative) != checksum:
            raise RuntimeError("Generated SDK layout override changed during compilation")
    check_sdk_worktree(sdk, profile)
    verify_toolchain(toolchain)
    if profile != "baseline" and rust_compiler(rust_env) != compiler:
        raise RuntimeError("Selected rustc changed during the build")
    layout_audit = audit_image(image_directory / "OTA_All.bin") if profile in MODEL_PROFILES else None
    pinmap_audit = None
    if profile in MODEL_PROFILES:
        km0_elf = sdk / PROJECT / "project_lp/asdk/image/target_img2.axf"
        symbols = run(str(toolchain / "bin/arm-none-eabi-nm"), "--defined-only",
                      str(km0_elf), capture_output=True, text=True).stdout
        matches = [line.split()[0] for line in symbols.splitlines()
                   if len(line.split()) == 3 and line.split()[-1] == "pmap_func"]
        if len(matches) != 1:
            raise RuntimeError("Expected one linked KM0 pmap_func symbol")
        pinmap_audit = pinmap.audit_linked_table(ota, int(matches[0], 16), STOCK.read_bytes())
    objects = {"KM0": archives.get("lp"), "KM4": archives.get("hp")}
    ram_park_audit = rampark.audit(sdk, PROJECT, toolchain / "bin", objects) if profile != "baseline" else None
    sdk_hook_audit = sdkhooks.audit(sdk, PROJECT, toolchain / "bin", objects) if profile != "baseline" else None
    startup_audit = firstboot.audit(sdk, PROJECT, toolchain / "bin") if profile == "plaf103-firstboot" else None
    manifest = {
        "profile": profile, "sdk_revision": LOCK["sdk"]["revision"],
        "startup_identity": identity if profile != "baseline" else None,
        "startup_output_policy": "stop-only" if profile != "baseline" else None,
        "startup_led_policy": "bounded-probe-v3" if profile != "baseline" else None,
        "startup_gpio_routing_policy": "read-only-v2" if profile != "baseline" else None,
        "startup_shared_pad_policy": "before-km4-v3" if profile == "plaf103-firstboot" else None,
        "startup_gpio_clock_policy": "explicit-enable-v1" if profile != "baseline" else None,
        "startup_flash_handshake_policy": "fw15-settings-v1" if profile != "baseline" else None,
        "toolchain_sha256": LOCK["toolchain"]["sha256"],
        "built_at": datetime.now(timezone.utc).isoformat(),
        "inputs": {str(p.relative_to(ROOT)): checksum for p, checksum in expected.items()},
        "sdk_overrides": overrides,
        "compiled_archives": compiled_archives,
        "rust_toolchain": compiler["version"] if compiler else None,
        "rust_compiler": compiler,
        "toolchain_files": toolchain_lock(),
        "layout_audit": layout_audit,
        "km0_pinmap_audit": pinmap_audit,
        "ram_park_audit": ram_park_audit,
        "sdk_hook_audit": sdk_hook_audit,
        "firstboot_startup_audit": startup_audit,
        "bootloader_replacement_supported": False,
        "artifacts": artifacts,
        "checks": {"ota_payload_matches": True, "ota_checksum_matches": True,
                   "bringup_banner_present": banner_present,
                   "required_core_symbols": required_symbols,
                   "source_snapshot_unchanged": True,
                   "sdk_worktree_within_allowlist": True,
                   "staged_sources_without_diagnostics": True},
        "device_compatibility": "Unverified SDK development image; desktop build only",
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Build complete; manifest: {output / 'manifest.json'}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("source-id", "bootstrap", "baseline", "bringup", *MODEL_PROFILES))
    args = parser.parse_args()
    try:
        if args.command == "source-id":
            profile = "plaf103-firstboot"
            print(source_identity(profile, {p: digest(p) for p in source_inputs(profile)}))
            return
        os.umask(0o077)
        toolchain = bootstrap()
        if args.command != "bootstrap":
            build(args.command, toolchain)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
