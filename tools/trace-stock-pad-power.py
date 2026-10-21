#!/usr/bin/env python3
"""Execute the stock KM0 shared-pad supply initializer offline, without device I/O."""

import argparse
import hashlib
import importlib.metadata
import json
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE, UC_HOOK_MEM_WRITE
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[1]
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
STOCK_SHA = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498"
EXIT = 0x0C000100
FUNCTION = 0x0C000328
CONTROL = 0x48000344

if not __debug__:
    raise RuntimeError("Evidence checks require assertions; do not use Python -O")


def run(image, trim=0x2A, initial=0, enabled=1, sleep_type=0):
    """Model register storage and the ROM delay only; execute the actual initializer/LDO code."""
    u = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    u.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    for base, size in [(0x0C000000, 0x20000), (0x80000, 0x10000),
                       (0x48000000, 0x1000), (0x1000, 0x1000)]:
        u.mem_map(base, size)
    u.mem_write(0x0C000020, bytes(image[0x40:0x14900]))
    u.mem_write(0x83000, bytes(image[0x14920:0x16DA0]))

    def write(address, value):
        u.mem_write(address, struct.pack("<I", value))

    def read(address):
        return struct.unpack("<I", u.mem_read(address, 4))[0]

    # Explicit synthetic power-management inputs, not inferred live state.
    write(0x84AE4, sleep_type)
    write(0x8506C, enabled)
    write(0x85878, trim)
    write(CONTROL, initial)
    write(0x48000208, 0x001115F7)
    write(0x48000280, 0xA0)
    writes, delays = [], []

    def code(uc, address, size, data):
        if address == 0x14C2:
            delays.append(uc.reg_read(arm.UC_ARM_REG_R0))
            uc.reg_write(arm.UC_ARM_REG_PC, uc.reg_read(arm.UC_ARM_REG_LR))
            return
        allowed = ((FUNCTION, 0x0C00034A), (0x0C011F78, 0x0C011F82),
                   (0x8450C, 0x845AE))
        if not any(start <= address < end for start, end in allowed):
            raise AssertionError(f"Unexpected stock execution: {address:#x}")

    def memory(uc, access, address, size, value, data):
        if address >= 0x48000000:
            writes.append({"address": hex(address), "size": size, "value": hex(value)})

    u.hook_add(UC_HOOK_CODE, code)
    u.hook_add(UC_HOOK_MEM_WRITE, memory)
    u.reg_write(arm.UC_ARM_REG_SP, 0x8F000)
    u.reg_write(arm.UC_ARM_REG_LR, EXIT | 1)
    u.emu_start(FUNCTION | 1, EXIT, timeout=1_000_000, count=1000)
    assert u.reg_read(arm.UC_ARM_REG_PC) == EXIT, "Initializer did not return"
    assert u.reg_read(arm.UC_ARM_REG_SP) == 0x8F000, "Unbalanced stock stack"
    return {"synthetic_trim": trim, "initial_control": hex(initial),
            "enabled": enabled, "sleep_type": sleep_type,
            "final_control": hex(read(CONTROL)),
            "final_function_enable": hex(read(0x48000208)),
            "writes": writes, "delay_requests_us": delays}


def audit(image=None):
    image = STOCK.read_bytes() if image is None else image
    if hashlib.sha256(image).hexdigest() != STOCK_SHA:
        raise ValueError("Requires the pinned stock OTA")
    # Stock main calls the reviewed function before entering its scheduler.
    offset = 0x40 + 0x0C00052A - 0x0C000020
    assert image[offset:offset + 4] == bytes.fromhex("fff7fdfe")
    cases = []
    for trim in (0x2A, 0x68):
        result = run(image, trim=trim, initial=0x80000000)
        assert int(result["final_control"], 16) == 0x80000301 | (trim << 1)
        assert int(result["final_function_enable"], 16) == 0x101115F7
        assert result["delay_requests_us"] == [5]
        assert [w["address"] for w in result["writes"]] == [
            "0x48000208", "0x48000344", "0x48000280",
            "0x48000344", "0x48000344", "0x48000344"]
        cases.append(result)
    for options in ({"enabled": 0}, {"sleep_type": 1}, {"initial": 0x355}):
        result = run(image, **options)
        assert len(result["writes"]) == 2 and not result["delay_requests_us"]
        cases.append(result)
    return {"stock_sha256": STOCK_SHA,
            "tool_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "unicorn": importlib.metadata.version("unicorn"),
            "stock_main_callsite": "0x0c00052a", "initializer": hex(FUNCTION),
            "cases": cases,
            "limits": {"physical_supply_measured": False, "pad_membership_proved": False,
                       "full_startup_emulated": False, "device_io": False},
            "model": "Register storage and intercepted ROM delay; explicit synthetic trim and PM state"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    output = args.report.resolve()
    if not output.is_relative_to(ROOT / "private") or output.exists():
        raise ValueError("Use a new report path beneath private/")
    report = audit()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Verified {len(report['cases'])} offline shared-pad supply cases; no device I/O")
