#!/usr/bin/env python3
"""Offline vendor-helper sector-program semantics; never opens a device.

Runs actual helper instructions with modeled NOR erase/program calls. Does not
establish physical write acceptance, protection state or silicon ROM behavior.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
from unicorn import UC_PROT_READ, UC_PROT_EXEC
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin"
HELPER_SHA = "9307121385cb390dfd2da64da2c6c515f17b5a9556b3d04021487c9b9f220b55"
EXIT, BUFFER, STATE = 0x8F000, 0x8C000, 0x83250
TARGET = 0x186000
CAPACITY = 0x800000


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def execute(helper, descending=False, explicit_erase=False, auto_erase_variant=False):
    require(len(helper) == 4688 and digest(helper) == HELPER_SHA, "Pinned helper required")
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(0, 0x10000)
    uc.mem_map(0x80000, 0x10000)
    uc.mem_write(0x82000, helper)
    uc.mem_map(0x08000000, CAPACITY, UC_PROT_READ | UC_PROT_EXEC)
    before = bytearray(b"\xa5" * CAPACITY)
    before[TARGET:TARGET + 0x1000] = b"\0" * 0x1000
    uc.mem_write(0x08000000, bytes(before))
    payload = bytes((i * 7 + 3) & 255 for i in range(4096))
    highwater = 0x2D6000 if descending else 0
    uc.mem_write(STATE, struct.pack("<I", highwater))
    calls = []
    if explicit_erase:
        uc.mem_write(0x08000000 + TARGET, b"\xff" * 4096)
        calls.append({"name": "explicit_sector_erase_fixture", "offset": TARGET, "bytes": 4096})

    def returned():
        uc.reg_write(arm.UC_ARM_REG_R0, 0)
        uc.reg_write(arm.UC_ARM_REG_PC, uc.reg_read(arm.UC_ARM_REG_LR))

    def hook(_uc, pc, _size, _data):
        args = [uc.reg_read(getattr(arm, f"UC_ARM_REG_R{i}")) for i in range(3)]
        if pc == EXIT:
            uc.emu_stop()
        elif pc == 0x2146:
            require(args[:2] == [2, TARGET], "Helper erase escapes the requested sector")
            calls.append({"name": "FLASH_Erase", "type": args[0], "offset": args[1], "bytes": 4096})
            uc.mem_write(0x08000000 + TARGET, b"\xff" * 4096)
            returned()
        elif pc == 0x2048:
            offset, size, pointer = args
            require(size == 8 and TARGET <= offset and offset + size <= TARGET + 4096,
                    "Helper program escapes the requested sector")
            require(BUFFER <= pointer and pointer + size <= BUFFER + 1024, "Program buffer escaped packet")
            data = bytes(uc.mem_read(pointer, size))
            existing = bytes(uc.mem_read(0x08000000 + offset, size))
            uc.mem_write(0x08000000 + offset, bytes(a & b for a, b in zip(existing, data)))
            calls.append({"name": "FLASH_TxData12B", "offset": offset, "bytes": size})
            returned()
        else:
            allowed = (0x820AC <= pc < 0x82150) if auto_erase_variant else (
                0x82284 <= pc < 0x8228C or 0x82154 <= pc < 0x82198)
            require(allowed, f"Unexpected program-path instruction: {pc:#x}")

    uc.hook_add(UC_HOOK_CODE, hook)
    for packet in range(4):
        uc.mem_write(BUFFER, payload[packet * 1024:(packet + 1) * 1024])
        for register, value in ((arm.UC_ARM_REG_R0, BUFFER),
                                (arm.UC_ARM_REG_R1, 0x08000000 + TARGET + packet * 1024),
                                (arm.UC_ARM_REG_R2, 1024), (arm.UC_ARM_REG_SP, 0x8E000),
                                (arm.UC_ARM_REG_LR, EXIT | 1)):
            uc.reg_write(register, value)
        uc.emu_start(0x820AD if auto_erase_variant else 0x82285, EXIT + 2,
                     count=10000, timeout=1_000_000)
        require(uc.reg_read(arm.UC_ARM_REG_PC) == EXIT, "Program wrapper did not return")
    after = bytes(uc.mem_read(0x08000000, CAPACITY))
    require(after[:TARGET] == before[:TARGET] and after[TARGET + 4096:] == before[TARGET + 4096:],
            "Synthetic flash changed outside target sector")
    writes = [c for c in calls if c["name"] == "FLASH_TxData12B"]
    require(len(writes) == 512 and [c["offset"] for c in writes] == list(range(TARGET, TARGET + 4096, 8)),
            "Unexpected program coverage")
    restored = after[TARGET:TARGET + 4096] == payload
    require(restored == (explicit_erase or (auto_erase_variant and not descending)),
            "Unexpected modeled NOR/auto-erase outcome")
    return {"program_entry": "0x820ac" if auto_erase_variant else "0x82284 -> 0x82154",
            "auto_erase_variant_fixture": auto_erase_variant,
            "descending_address_fixture": descending, "explicit_erase_fixture": explicit_erase,
            "initial_highwater": hex(highwater), "helper_auto_sector_erases": sum(c["name"] == "FLASH_Erase" for c in calls),
            "eight_byte_program_calls": len(writes), "programmed_bytes": 4096,
            "target_matches_payload": restored, "outside_target_unchanged": True,
            "final_sector_sha256": digest(after[TARGET:TARGET + 4096])}


def audit():
    helper = HELPER.read_bytes()
    return {"created_utc": datetime.now(timezone.utc).isoformat(), "helper_sha256": digest(helper),
            "active_program_wrapper": "0x82284 -> 0x82154", "alternate_auto_erase_wrapper": "0x820ac",
            "erase_highwater_state_for_alternate_wrapper": hex(STATE),
            "cases": [execute(helper), execute(helper, explicit_erase=True),
                      execute(helper, auto_erase_variant=True),
                      execute(helper, True, False, True), execute(helper, True, True, True)],
            "conclusion": "The active packet path selects a no-auto-erase wrapper; every restored sector needs explicit erase.",
            "limits": ["Only program-wrapper instructions executed; packet parser/transport not executed here",
                       "ROM erase/program calls and NOR behavior modeled; physical acceptance/protection unverified",
                       "Explicit erase is a supplied fixture, not execution of the helper erase-command parser",
                       "No live writer is implemented or device operation performed"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    require(args.report.resolve().is_relative_to((ROOT / "private").resolve()), "Report must be private")
    report = audit()
    args.report.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with args.report.open("x") as handle:
        handle.write(json.dumps(report, indent=2) + "\n")
    print(report["conclusion"])


if __name__ == "__main__":
    main()
