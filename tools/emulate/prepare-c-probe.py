#!/usr/bin/env python3
"""Build the fixed non-actuating stock execution probe offline; never contacts a feeder.

The probe is Rust (`firmware/rust/probe`), compiled for soft-float Cortex-M4 and
linked by the pinned ASDK linker with `firmware/probe/probe.ld`.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
PROBE = ROOT / "firmware/probe"
RUST_PROBE = ROOT / "firmware/rust/probe"
RUST_TARGET = "thumbv7em-none-eabi"
TOOLS = ROOT / "private/toolchains/asdk-6.4.1/linux/newlib/bin"
SLOT_OFFSET = 0x14FCA0
SLOT_ADDRESS = 0x0E138C80
SLOT_BYTES = 512
HOOK_ADDRESS = 0x0E014246
HOOK_OFFSET = 0x17040 + HOOK_ADDRESS - 0x0E000020
HOOK_ORIGINAL = bytes.fromhex("f7f713fd")
VERSION_OFFSET = 0xEDC95
APIS = {"stock_json_add": 0x0E00BC71, "stock_json_find": 0x0E00BC3F,
        "stock_json_string": 0x0E00BCFB, "stock_json_duplicate": 0x0E00B145,
        "stock_json_append": 0x0E00BC59, "stock_json_delete": 0x0E00B821}
SPEC = importlib.util.spec_from_file_location("ota_layout", ROOT / "tools/audit/audit-ota-layout.py")
OTA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OTA)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def thumb_bl(source, target):
    delta = (target & ~1) - (source + 4)
    OTA.require(delta % 2 == 0 and -(1 << 24) <= delta < (1 << 24),
                "Thumb branch is out of range or unaligned")
    s = (delta >> 24) & 1
    j1 = 1 ^ ((delta >> 23) & 1) ^ s
    j2 = 1 ^ ((delta >> 22) & 1) ^ s
    return struct.pack("<HH", 0xF000 | (s << 10) | ((delta >> 12) & 1023),
                       0xD000 | (j1 << 13) | (j2 << 11) | ((delta >> 1) & 2047))


def apply_patch(original, code):
    OTA.require(digest(original) == OTA.STOCK_SHA256, "Not the verified stock image")
    OTA.require(0 < len(code) <= SLOT_BYTES, "Probe does not fit the reviewed slot")
    OTA.require(original[HOOK_OFFSET:HOOK_OFFSET + 4] == HOOK_ORIGINAL,
                "Unexpected original GET_CONFIG call")
    OTA.require(thumb_bl(HOOK_ADDRESS, APIS["stock_json_add"]) == HOOK_ORIGINAL,
                "Branch encoder disagrees with the original instruction")
    OTA.require(original[VERSION_OFFSET:VERSION_OFFSET + 7] == b"1.1.15\0",
                "Unexpected original version")
    OTA.require(original[SLOT_OFFSET:SLOT_OFFSET + SLOT_BYTES] == b"\xff" * SLOT_BYTES,
                "Probe slot is not entirely format padding")
    reference = OTA.regions(OTA.container(original))
    last = reference[-1]
    padding_start = 32 + last["payload_header_offset"] + 32 + last["body_bytes"]
    OTA.require(SLOT_OFFSET >= padding_start and SLOT_OFFSET + SLOT_BYTES <= len(original),
                "Probe is outside final format padding")
    km4 = reference[2]
    OTA.require(SLOT_ADDRESS == 0x0E000000 + SLOT_OFFSET -
                (32 + km4["payload_header_offset"]), "Probe virtual/file mapping mismatch")
    result = bytearray(original)
    result[VERSION_OFFSET:VERSION_OFFSET + 7] = b"1.1.17\0"
    result[HOOK_OFFSET:HOOK_OFFSET + 4] = thumb_bl(HOOK_ADDRESS, SLOT_ADDRESS)
    result[SLOT_OFFSET:SLOT_OFFSET + len(code)] = code
    struct.pack_into("<I", result, 16, sum(result[32:]) & 0xFFFFFFFF)
    return bytes(result)


def verify_patch(original, candidate, code):
    OTA.require(candidate == apply_patch(original, code),
                "Unexpected change outside the exact reviewed patch")
    payload = bytearray(OTA.container(candidate))
    # The ordinary auditor requires erased final padding. Explicitly account for
    # this one slot, then check that every load header and body size is unchanged.
    payload[SLOT_OFFSET - 32:SLOT_OFFSET - 32 + len(code)] = b"\xff" * len(code)
    OTA.require(OTA.regions(payload) == OTA.regions(OTA.container(original)),
                "Load headers or image sizes changed")
    return {"same_file_and_bank_erase_size": True, "same_load_headers": True,
            "hook_file_offset": hex(HOOK_OFFSET), "hook_runtime_address": hex(HOOK_ADDRESS),
            "hook_before": HOOK_ORIGINAL.hex(),
            "hook_after": candidate[HOOK_OFFSET:HOOK_OFFSET + 4].hex(),
            "probe_file_offset": hex(SLOT_OFFSET), "probe_runtime_address": hex(SLOT_ADDRESS),
            "probe_bytes": len(code), "probe_slot_bytes": SLOT_BYTES,
            "changed_bytes": sum(a != b for a, b in zip(original, candidate))}


def build(output):
    output = output.resolve()
    OTA.require(output.is_relative_to((ROOT / "private").resolve()), "Output must be private")
    output.mkdir(parents=True, exist_ok=False)
    inputs = [PROBE / "probe.ld", RUST_PROBE / "Cargo.toml", RUST_PROBE / "src/lib.rs"]
    inputs += [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "rust-toolchain.toml",
               Path(__file__).resolve(), ROOT / "firmware/sdk.lock.json",
               PROBE / "toolchain.lock.json",
               ROOT / "firmware/plaf103-layout.json",
               ROOT / "tools/audit/audit-ota-layout.py"]
    hashes = {str(path.relative_to(ROOT)): digest(path.read_bytes()) for path in inputs}
    staged = output / "inputs"
    staged.mkdir()
    for path in inputs[:3]:
        shutil.copyfile(path, staged / path.name)
    original = STOCK.read_bytes()
    OTA.audit(STOCK, STOCK, ROOT / "firmware/plaf103-layout.json")
    toolchain = json.loads((PROBE / "toolchain.lock.json").read_text())
    sdk = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
    OTA.require(toolchain["archive_sha256"] == sdk["toolchain"]["sha256"],
                "Probe toolchain pin does not match SDK archive")

    def verify_tools():
        for name, expected in toolchain["files"].items():
            path = (TOOLS.parent / name).resolve()
            OTA.require(path.is_relative_to(TOOLS.parent.resolve()), "Tool escaped pinned directory")
            OTA.require(digest(path.read_bytes()) == expected, "Pinned compiler tool changed: " + name)

    verify_tools()
    environment = os.environ.copy()
    for key in ["GCC_EXEC_PREFIX", "COMPILER_PATH", "LIBRARY_PATH", "CPATH",
                "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH"]:
        environment.pop(key, None)

    def run(tool, *args):
        result = subprocess.run([str(TOOLS / ("arm-none-eabi-" + tool)), *map(str, args)],
                                text=True, capture_output=True, env=environment)
        if result.returncode:
            raise ValueError(f"{tool} failed ({result.returncode}): {result.stderr.strip()}")
        return result.stdout

    elf = output / "probe.elf"
    cargo_environment = os.environ.copy()
    for key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_DIR"]:
        cargo_environment.pop(key, None)
    cargo = ["cargo", "build", "--locked", "--profile", "firmware", "-p", "wellfed-probe",
             "--target", RUST_TARGET, "--target-dir", str(output / "rust-target")]
    result = subprocess.run(cargo, cwd=ROOT, text=True, capture_output=True, env=cargo_environment)
    if result.returncode:
        raise ValueError(f"cargo failed ({result.returncode}): {result.stderr.strip()}")
    archive = output / "rust-target" / RUST_TARGET / "firmware/libwellfed_probe.a"
    command = ["-mcpu=cortex-m4", "-mthumb", "-mfloat-abi=soft", "-nostdlib",
               "-Wl,--gc-sections,--fatal-warnings", "-Wl,--undefined=probe_add_version",
               "-Wl,-Map=" + str(output / "probe.map"),
               "-T", str(staged / "probe.ld"), str(archive), "-o", str(elf)]
    run("gcc", *command)
    OTA.require(not run("nm", "-u", elf).strip(), "Probe has unresolved symbols")
    symbols = {}
    for line in run("nm", "-n", elf).splitlines():
        match = re.match(r"^([0-9a-fA-F]+)\s+\w\s+(\S+)$", line)
        if match:
            symbols[match[2]] = int(match[1], 16)
    OTA.require(symbols["probe_add_version"] == SLOT_ADDRESS, "Unexpected probe entry")
    for name, address in APIS.items():
        OTA.require(symbols[name] == address, "Stock API address changed: " + name)
    code_path = output / "probe.bin"
    run("objcopy", "-O", "binary", "--only-section=.probe", elf, code_path)
    code = code_path.read_bytes()
    assembly = run("objdump", "-d", elf)
    (output / "probe.asm").write_text(assembly)
    calls = []
    for line in assembly.splitlines():
        if re.search(r"\bblx\b", line):
            raise ValueError("Indirect call in probe")
        match = re.search(r"\bbl\s+([0-9a-fA-F]+)", line)
        if match:
            target = int(match[1], 16)
            OTA.require(target in {a & ~1 for a in APIS.values()} or
                        SLOT_ADDRESS <= target < SLOT_ADDRESS + len(code),
                        "Unexpected probe call target")
            calls.append(hex(target))
    candidate = apply_patch(original, code)
    audit = verify_patch(original, candidate, code)
    for path in inputs:
        OTA.require(digest(path.read_bytes()) == hashes[str(path.relative_to(ROOT))],
                    "Build input changed while compiling")
    verify_tools()
    (output / "original-1.1.15.bin").write_bytes(original)
    (output / "c-probe-1.1.17.bin").write_bytes(candidate)
    report = {"kind": "stock-c-execution-probe-v1", "created_at": datetime.now(timezone.utc).isoformat(),
              "original_sha256": digest(original), "candidate_sha256": digest(candidate),
              "probe_sha256": digest(code), "bytes": len(candidate), "audit": audit,
              "language": "rust", "rust_target": RUST_TARGET,
              "rustc": subprocess.run(["rustc", "--version"], cwd=ROOT, text=True,
                                      capture_output=True, check=True).stdout.strip(),
              "cargo_args": cargo[1:], "archive_sha256": digest(archive.read_bytes()),
              "linker": run("gcc", "--version").splitlines()[0], "linker_args": command,
              "tool_sha256": {name: digest((TOOLS / ("arm-none-eabi-" + name)).read_bytes())
                              for name in ["gcc", "as", "ld", "objcopy", "objdump", "nm"]},
              "source_sha256": hashes, "calls": calls,
              "device_command_sent": False, "emulated": False,
              "actual_loader_and_mpu_verified": False, "physical_execution_verified": False}
    (output / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(build(args.output), indent=2))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
