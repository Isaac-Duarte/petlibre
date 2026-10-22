#!/usr/bin/env python3
"""Check stock startup MPU configuration offline; actual loader and silicon remain unverified."""

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE, UC_PROT_READ, UC_PROT_EXEC
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("probe_patch", ROOT / "tools/emulate/prepare-c-probe.py")
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)
ROM = ROOT / "private/firmware/plaf103-sdk-rom-inputs/km4-rom-text.bin"
ROM_SHA256 = "073a80be644d776a007cd22fc4c10490ce75655de450c6f65849b254f2140fa9"
MPU = 0xE000ED90
STACK = 0x100F0000
EXIT = 0x0E000100

if not __debug__:
    raise RuntimeError("Audit requires assertions; do not use Python -O")


def file_offset(address):
    if 0x0E000020 <= address < 0x0E11E050:
        return 0x17040 + address - 0x0E000020
    if 0x10005000 <= address < 0x1001FBE0:
        return 0x135090 + address - 0x10005000
    raise ValueError("Address is outside initialized stock KM4 bodies")


def startup_evidence(image):
    def instruction(address, count):
        return image[file_offset(address):file_offset(address) + count]

    assert instruction(0x10005000, 4) == struct.pack("<I", 0x0E02C2E1)
    for source, target in [(0x0E02C428, 0x0E0D49E0), (0x0E02C42C, 0x0E02C018)]:
        assert instruction(source, 4) == PATCH.thumb_bl(source, target), "startup MPU calls changed"
    thunks = {0x0E0D49E0: 0x1010CC01, 0x0E0D4830: 0x1010CCB1, 0x0E0D5720: 0x1010CDC1}
    for address, target in thunks.items():
        assert instruction(address, 12) == bytes.fromhex("01b40248844601bc604700bf")
        assert instruction(address + 12, 4) == struct.pack("<I", target)
    control = []
    for start, length, base in [(0x17040, 1171504, 0x0E000020), (0x135090, 109536, 0x10005000)]:
        for at in range(start, start + length - 3, 2):
            first, second = struct.unpack_from("<HH", image, at)
            if first & 0xFFF0 == 0xF380 and second == 0x8814:
                control.append(base + at - start)
    assert control == [0x1000D5AE], "unexpected CONTROL-write encoding; privilege needs new analysis"
    assert instruction(0x1000D5AC, 6) == bytes.fromhex("022181f31488"), "first task no longer selects CONTROL=2"
    return {"application_entry": "0x0e02c2e0", "app_mpu_configuration": "0x0e02c018",
            "rom_init_target": "0x1010cc00", "initialized_km4_control_write_encodings": [hex(a) for a in control],
            "first_task_control_literal": 2, "first_task_npriv_bit": 0,
            "scope": "Aligned instruction encodings in initialized KM4 bodies, not a whole-device privilege proof"}


def execute_configuration(image, rom, cache_enabled, occupied):
    assert cache_enabled in [0, 1] and 0 <= occupied <= 3
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(0x0E000000, 0x200000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(0x0E000000, image[0x17020:])
    uc.mem_map(0x10000000, 0x100000)
    uc.mem_write(0x10005000, image[0x135090:0x14FC70])
    uc.mem_map(0x10100000, 0x40000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(0x1010A000, rom)
    # This is ordinary backing memory, not an emulation of MPU enforcement.
    uc.mem_map(0xE000E000, 0x2000)
    uc.mem_write(0x1001EDE4, struct.pack("<I", cache_enabled))
    uc.mem_write(0x10001340, b"\x01" * occupied + b"\0" * (8 - occupied))
    regions = []
    pending = None
    expected_registers = {getattr(arm, f"UC_ARM_REG_R{n}"): 0xA4000000 + n for n in range(4, 12)}

    def word(address):
        return struct.unpack("<I", uc.mem_read(address, 4))[0]

    def on_code(_uc, address, _size, _data):
        nonlocal pending
        if pending is not None and address == pending[0]:
            request = pending[1]
            assert word(MPU + 8) == request["index"]
            request.update({"rbar": word(MPU + 12), "rlar": word(MPU + 16)})
            assert request["rbar"] == (request["base"] & ~31) | request["xn"] | request["ap"] | request["sh"]
            assert request["rlar"] == ((request["base"] + request["bytes"]) & ~31) | (request["attr"] << 1) | 1
            regions.append(request)
            pending = None
        if address == 0x1010CCB0:
            assert pending is None
            base, size, xn, ap, sh, attr = struct.unpack("<II4B", uc.mem_read(uc.reg_read(arm.UC_ARM_REG_R1), 12))
            pending = (uc.reg_read(arm.UC_ARM_REG_LR) & ~1,
                       {"index": uc.reg_read(arm.UC_ARM_REG_R0), "base": base, "bytes": size,
                        "xn": xn, "ap": ap, "sh": sh, "attr": attr})
        allowed = (address == EXIT or 0x0E02C018 <= address < 0x0E02C0FC
                   or any(start <= address < start + 12 for start in [0x0E0D49E0, 0x0E0D4830, 0x0E0D5720])
                   or 0x1010CC00 <= address < 0x1010CDE8)
        assert allowed, f"unexpected configuration execution {address:#x}"

    uc.hook_add(UC_HOOK_CODE, on_code)
    for entry in [0x0E0D49E1, 0x0E02C019]:
        for register, value in expected_registers.items():
            uc.reg_write(register, value)
        uc.reg_write(arm.UC_ARM_REG_SP, STACK)
        uc.reg_write(arm.UC_ARM_REG_LR, EXIT | 1)
        uc.emu_start(entry, EXIT, timeout=1_000_000, count=4000)
        assert uc.reg_read(arm.UC_ARM_REG_PC) == EXIT, "configuration did not return"
        assert uc.reg_read(arm.UC_ARM_REG_SP) == STACK
        for register, value in expected_registers.items():
            assert uc.reg_read(register) == value, "configuration clobbered a preserved register"
        assert word(MPU + 4) == 5, "candidate ROM MPU control lacks expected ENABLE/PRIVDEFENA"
    expected_first = (0x10030460, 0x5D60) if cache_enabled else (0x10000000, 0x80000)
    assert [(r["base"], r["bytes"]) for r in regions] == [expected_first, (0x1010A000, 0x36000),
        (0x101C0000, 0x14000), (0x000C0000, 0x400), (0x02000000, 0x400000)]
    assert [r["index"] for r in regions] == list(range(occupied, occupied + 5))
    assert all(r["xn"] == 0 and r["ap"] == 2 and r["sh"] == 0 for r in regions)
    assert not any(r["base"] <= PATCH.SLOT_ADDRESS < r["base"] + r["bytes"] for r in regions)
    return {"cache_enabled_fixture": bool(cache_enabled), "initial_allocator_entries_reserved_fixture": occupied,
            "regions_added_by_application": regions, "control": word(MPU + 4),
            "mair0": word(MPU + 0x30), "probe_region_added": False}


def audit():
    image = PATCH.STOCK.read_bytes()
    assert PATCH.digest(image) == PATCH.OTA.STOCK_SHA256, "not the pinned stock image"
    rom = ROM.read_bytes()
    assert len(rom) == 151280 and PATCH.digest(rom) == ROM_SHA256, "candidate SDK ROM changed"
    return {"created_at": datetime.now(timezone.utc).isoformat(), "stock_sha256": PATCH.digest(image),
            "candidate_sdk_rom_sha256": ROM_SHA256, "startup": startup_evidence(image),
            "cases": [execute_configuration(image, rom, cache, reserved) for cache, reserved in [(0, 0), (1, 0), (1, 3)]],
            "tool_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "unicorn_version": importlib.metadata.version("unicorn"),
            "limits": {"actual_rom_bytes_verified": False, "factory_loader_mapping_verified": False,
                       "initial_or_later_mpu_regions_verified": False, "trustzone_state_verified": False,
                       "mpu_enforcement_emulated": False, "actual_probe_execution_verified": False},
            "conclusion": "The inspected application routine adds no region covering the probe; candidate ROM enables privileged background access. Factory mapping and live state remain unknown."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    try:
        assert not os.path.lexists(args.report), "Report already exists"
        path = args.report.resolve()
        assert path.is_relative_to((ROOT / "private").resolve()), "Report must be private"
        report = audit()
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as handle:
            json.dump(report, handle, indent=2)
            handle.write("\n")
        print(f"Offline startup checks passed; hardware mapping remains unverified. Report: {path}")
    except (OSError, ValueError, AssertionError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
