#!/usr/bin/env python3
"""Offline execution of the verified research feeder's factory flash mapper.

Actual loader bytes run with candidate ROM RSIP routines. MMIO is ordinary RAM;
virtual header access is supplied from the requested physical flash range.
No device connection or flash operation is performed.
"""
import argparse
from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
from unicorn import UC_PROT_READ, UC_PROT_EXEC
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("recovery_boot", ROOT / "tools/audit/audit-recovery-boot.py")
BOOT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BOOT)
ROM = ROOT / "private/upstream/ameba-rtos-d/project/realtek_amebaD_va0_example/GCC-RELEASE/project_lp/asdk/lib/rom_bcut/km0_irom.bin"
FLASH = ROOT / "private/fixtures/baseline.bin"
ORIGINAL = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
PROBE = ROOT / "private/firmware/experiments/rust-probe-1.1.17/c-probe-1.1.17.bin"
ORIGINAL_SHA = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498"
PROBE_SHA = "9af0ce801c538f7ff4b6edbe84f4d250791c5dedefbf01f0b18958b7123df6e7"
SLOT = 0x0E138C80
SLOT_BYTES = 512
BANKS = (0x6000, 0x186000)
EXIT = 0x8F000


def word(data, offset):
    return struct.unpack_from("<I", data, offset)[0]


def execute(flash, rom, bank=None):
    """Run bank check, or the ordinary bank selector, from actual instructions.

These are supplied-state function/slice tests, not execution of the full reset.
Original factory configuration is loaded; earlier MMIO/cache setup is modeled.
"""
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(0, 0x10000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(0, rom)
    uc.mem_map(0x08000000, 0x800000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(0x08000000, bytes(flash))
    uc.mem_map(0x80000, 0x10000)
    uc.mem_write(0x82000, bytes(flash[0x720:0x1030]))
    for address in (0x0C000000, 0x0E000000):
        uc.mem_map(address, 0x200000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_map(0x48000000, 0x1000)
    uc.mem_map(0xE000E000, 0x2000)
    uc.reg_write(arm.UC_ARM_REG_SP, 0x8E000)
    uc.reg_write(arm.UC_ARM_REG_LR, EXIT | 1)
    uc.reg_write(arm.UC_ARM_REG_R0, bank or 0)
    uc.reg_write(arm.UC_ARM_REG_R4, 0x801CC)
    configs, commands, outcome = [], [], {}
    pending = None

    def read32(address):
        return word(uc.mem_read(address, 4), 0)

    def hook(_uc, pc, _size, _data):
        nonlocal pending
        if pending is not None and pc == pending[0]:
            request = pending[1]
            base = 0x48000630 + request["index"] * 16
            registers = [read32(base + offset) for offset in (0, 4, 8, 12)]
            BOOT.require(registers[1:] == [request["start"], request["end"], request["offset"]],
                         "Candidate ROM mapping registers disagree with request")
            BOOT.require(registers[0] & 2 == request["minus"] * 2, "Wrong mapping direction")
            request["registers"] = registers
            configs.append(request)
            physical = request["start"] + (-request["offset"] if request["minus"] else request["offset"])
            BOOT.require(0x08000000 <= physical < 0x08800000, "Mapping escapes external flash")
            length = min(request["end"] - request["start"] + 1, 0x200000, 0x08800000 - physical)
            uc.mem_write(request["start"], bytes(flash[physical - 0x08000000:physical - 0x08000000 + length]))
            pending = None
        if pc == 0x3624:
            values = [uc.reg_read(getattr(arm, f"UC_ARM_REG_R{i}")) for i in range(4)]
            request = dict(zip(("index", "start", "end", "minus"), values))
            request["offset"] = read32(uc.reg_read(arm.UC_ARM_REG_SP))
            BOOT.require(request["index"] in (0, 1) and request["start"] in (0x0C000000, 0x0E000000),
                         "Unexpected mapping index or virtual start")
            pending = (uc.reg_read(arm.UC_ARM_REG_LR) & ~1, request)
        elif pc == 0x3648:
            commands.append([uc.reg_read(arm.UC_ARM_REG_R0), uc.reg_read(arm.UC_ARM_REG_R1)])
        elif pc in (EXIT, 0x080004A0, 0x08000482, 0x08000182):
            outcome.update(stop=hex(pc), result=uc.reg_read(arm.UC_ARM_REG_R0))
            uc.emu_stop()
        elif not (0x08000020 <= pc < 0x080001AE or 0x080003F4 <= pc < 0x080004A0
                  or 0x3624 <= pc < 0x3642 or 0x3648 <= pc < 0x365E):
            raise ValueError(f"Unexpected factory-mapping instruction: {pc:#x}")

    uc.hook_add(UC_HOOK_CODE, hook)
    uc.emu_start(0x08000155 if bank is not None else 0x080003F5, EXIT + 2,
                 timeout=1_000_000, count=4000)
    BOOT.require(outcome and pending is None, "Loader did not reach a reviewed boundary")
    outcome.update(configs=configs, commands=commands,
                   final_registers=[[read32(0x48000630 + i * 16 + j) for j in (0, 4, 8, 12)] for i in (0, 1)])
    return outcome


def check_mapping(result, image, bank):
    BOOT.require(result["stop"] == hex(EXIT) and result["result"] == 1, "Valid bank check failed")
    mappings = result["configs"]
    BOOT.require(len(mappings) == 4 and result["commands"] == [[0, 1], [1, 1]], "Unexpected map sequence")
    for index in (0, 1):
        initial, final = mappings[index * 2:index * 2 + 2]
        BOOT.require(initial["end"] == initial["start"] + 0x1FFFFFF, "Missing temporary 32 MiB map")
        expected_start = (0x0C000000, 0x0E000000)[index]
        expected_size = (0x17000, 0x139000)[index]
        physical = 0x08000000 + BANKS[bank] + (0 if index == 0 else 0x17000)
        BOOT.require(final["start"] == expected_start and final["end"] == expected_start + expected_size - 1,
                     "Final factory mapping extent differs")
        BOOT.require(final["minus"] == 1 and final["offset"] == expected_start - physical,
                     "Final factory mapping translation differs")
        BOOT.require(result["final_registers"][index][0] == 3, "Mapping is not enabled/subtractive")
    physical_probe = SLOT - mappings[-1]["offset"]
    file_offset = 32 + physical_probe - (0x08000000 + BANKS[bank])
    BOOT.require(file_offset == 0x14FCA0 and SLOT + SLOT_BYTES - 1 <= mappings[-1]["end"],
                 "Probe slot is outside mapped image")
    return {"bank": bank, "slot_virtual": hex(SLOT), "slot_bytes": SLOT_BYTES,
            "slot_physical": hex(physical_probe), "slot_ota_file_offset": hex(file_offset),
            "slot_sha256": BOOT.digest(image[file_offset:file_offset + SLOT_BYTES]),
            "mapping": result}


def audit(flash, rom, original, probe):
    for data, expected, label in ((flash, BOOT.FLASH_SHA256, "full backup"), (rom, BOOT.ROM_SHA256, "ROM"),
                                  (original, ORIGINAL_SHA, "stock"), (probe, PROBE_SHA, "probe")):
        BOOT.require(BOOT.digest(data) == expected, "Unexpected " + label + " hash")
    BOOT.factory_segments(flash)
    BOOT.require(flash[BANKS[1]:BANKS[1] + len(original) - 32] == original[32:],
                 "Backup bank 1 does not match the original OTA payload")
    valid = check_mapping(execute(flash, rom, 1), original, 1)
    invalid = execute(flash, rom, 0)
    BOOT.require(invalid["result"] == 0 and invalid["stop"] == hex(EXIT), "Invalid marker accepted")
    actual = execute(flash, rom)
    BOOT.require(actual["result"] == 2 and actual["stop"] == "0x80004a0", "Actual backup selects unexpected bank")
    fixtures = []
    for image in (original, probe):
        for bank in (0, 1):
            modified = bytearray(flash)
            modified[BANKS[bank]:BANKS[bank] + len(image) - 32] = image[32:]
            fixtures.append(check_mapping(execute(modified, rom, bank), image, bank))
    fallback = bytearray(flash)
    fallback[BANKS[0]:BANKS[0] + len(original) - 32] = original[32:]
    fallback[BANKS[1]] = 0
    chosen = execute(fallback, rom)
    BOOT.require(chosen["result"] == 1 and chosen["stop"] == "0x80004a0", "Fallback bank selection differs")
    fallback[BANKS[0]] = 0
    neither = execute(fallback, rom)
    BOOT.require(neither["stop"] == "0x8000482", "Neither-valid case does not reach wait loop")
    return {"created_utc": datetime.now(timezone.utc).isoformat(), "backup_sha256": BOOT.digest(flash),
            "candidate_rom_sha256": BOOT.digest(rom), "actual_backup_valid_bank": valid,
            "actual_backup_invalid_bank": invalid, "actual_backup_selection": actual,
            "stock_and_probe_mapping_fixtures": fixtures, "fallback_selection_fixture": chosen,
            "neither_valid_fixture": neither,
            "conclusion": "Actual factory mapping includes the full probe slot in either bank; executable permission/live execution remain unverified.",
            "limits": ["Supplied-state loader functions/slice, not complete reset or hardware MMU emulation",
                       "Candidate ROM RSIP instructions execute, but device correspondence is sampled only",
                       "Only bounded virtual header contents are supplied; MPU/TrustZone/cache enforcement is not modeled",
                       "No live probe execution, physical recovery or accepted write-back established"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    BOOT.require(args.report.resolve().is_relative_to((ROOT / "private").resolve()), "Report must be private")
    report = audit(FLASH.read_bytes(), ROM.read_bytes(), ORIGINAL.read_bytes(), PROBE.read_bytes())
    args.report.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with args.report.open("x") as handle:
        handle.write(json.dumps(report, indent=2) + "\n")
    print(report["conclusion"])


if __name__ == "__main__":
    main()
