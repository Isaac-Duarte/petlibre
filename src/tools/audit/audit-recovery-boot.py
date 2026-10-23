#!/usr/bin/env python3
"""Offline factory-loader/ROM route audit for this research feeder's verified dump.

Never opens serial, changes a pin, writes flash or certifies physical recovery.
Original boot bodies and ROM remain ignored private artifacts.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[3]
FLASH_SHA256 = "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc"
ROM_SHA256 = "db0fdd8b2232a0d9edaa403715a9992ae36944fbda4dd936e017c1fa531e66d6"
FLASH_BYTES = 8 * 1024 * 1024
MAGIC = (0x96969999, 0xFC66CC3F)
SEGMENTS = (("km0_factory_flash", 0, 0x6E0, 0x08000020),
            ("km0_factory_ram", 0x700, 0x910, 0x00082000),
            ("km4_factory_flash", 0x4000, 0, 0x08004020),
            ("km4_factory_ram", 0x4020, 0x1030, 0x1007D000))


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def factory_segments(data):
    require(len(data) == FLASH_BYTES, "Expected exactly 8 MiB")
    result = []
    previous_end = 0
    for name, offset, length, address in SEGMENTS:
        header = struct.unpack_from("<8I", data, offset)
        require(header == MAGIC + (length, address) + (0xFFFFFFFF,) * 4,
                "Unexpected factory image header: " + name)
        start, end = offset + 32, offset + 32 + length
        require(offset >= previous_end and end <= 0x6000, "Factory range overlap/overflow")
        body = data[start:end]
        entry = {"name": name, "header_offset": hex(offset), "body_offset": hex(start),
                 "body_bytes": length, "load_address": hex(address), "body_sha256": digest(body)}
        if name.endswith("ram"):
            words = struct.unpack_from("<8I", body)
            require(words[0] & 1 and address <= (words[0] & ~1) < address + length,
                    "Factory RAM entry pointer escapes its body")
            entry["start_table_words"] = [hex(word) for word in words]
        result.append(entry)
        previous_end = end
    return result


def execute_selection(rom, strap_high, backup):
    """Execute actual candidate instructions from 0x4766 to either branch sink.

    Latched strap and backup registers are supplied in modeled MMIO RAM. The
    UART routine and remaining normal boot are stop points, not emulated I/O.
    This deliberately starts after earlier reset setup/optional patch hooks.
    """
    from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
    from unicorn import arm_const as arm
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(0, 0x10000)
    uc.mem_write(0, rom)
    uc.mem_map(0x80000, 0x10000)
    uc.mem_map(0x48000000, 0x1000)
    uc.mem_write(0x480003F0, struct.pack("<I", (1 << 30) if strap_high else 0))
    uc.mem_write(0x480003C0, struct.pack("<I", backup))
    uc.reg_write(arm.UC_ARM_REG_SP, 0x8FF00)
    outcome = {}

    def step(_uc, pc, _size, _data):
        if pc == 0x634C:
            outcome.update(route="uart_download", uart=uc.reg_read(arm.UC_ARM_REG_R0),
                           reached_before_factory_header_load=True)
            uc.emu_stop()
        elif pc == 0x4786:
            outcome.update(route="normal_boot_continuation")
            uc.emu_stop()
        elif not (0x4766 <= pc < 0x4786 or 0x50CC <= pc < 0x50D8 or
                  0x1434 <= pc < 0x144C or 0x147C <= pc < 0x149C):
            raise ValueError(f"Unexpected instruction in modeled selector: {pc:#x}")

    uc.hook_add(UC_HOOK_CODE, step)
    uc.emu_start(0x4767, 0x10000, count=300, timeout=1_000_000)
    require(outcome, "ROM selector did not reach a reviewed stop point")
    expected = "uart_download" if not strap_high or backup & 0x200 else "normal_boot_continuation"
    require(outcome["route"] == expected, "ROM selector disagrees with latched strap/backup cases")
    after = struct.unpack("<I", uc.mem_read(0x480003C0, 4))[0]
    require(after == (backup & ~0x200 if expected == "uart_download" else backup),
            "Unexpected backup-register side effect")
    if expected == "uart_download":
        require(outcome["uart"] == 2, "Unexpected download UART")
    return {"supplied_strap_high": strap_high, "supplied_backup": hex(backup),
            "backup_after": hex(after), **outcome}


def selection_cases(rom):
    require(len(rom) == 54144, "Unexpected candidate ROM size")
    return [execute_selection(rom, high, backup)
            for high in (True, False) for backup in (0, 0x200, 0x400, 0x208)]


def audit(flash, rom):
    require(digest(flash) == FLASH_SHA256, "Not the verified research-unit full dump")
    require(digest(rom) == ROM_SHA256, "Not the pinned sampled B-cut ROM candidate")
    require(struct.unpack_from("<I", rom, 0x50D8)[0] == 0x480003F0,
            "Unexpected trap-status register literal")
    return {"created_utc": datetime.now(timezone.utc).isoformat(),
            "flash_sha256": digest(flash), "candidate_rom_sha256": digest(rom),
            "factory_segments": factory_segments(flash),
            "candidate_rom_selector_cases": selection_cases(rom),
            "instruction_findings": {"selector_entry": "0x4766", "trap_helper": "0x50cc",
                                     "latched_status": "0x480003f0 bit 30",
                                     "software_request": "backup register 0 bit 9",
                                     "uart_download_call": "0x4778 -> 0x634c, UART 2",
                                     "later_factory_header_read": "0x47ec"},
            "scope": "Offline supplied-state execution of a candidate ROM selection slice; no device I/O",
            "limits": ["Only 68 device ROM words were previously sampled, not the whole reset path",
                       "Earlier reset setup, optional patch hooks and physical strap latching not modeled",
                       "No physical strap voltage, resistance, electrical entry or write-back verified",
                       "Do not ground a UART output based solely on this report",
                       "SDK header trap-pin commentary conflicts with manufacturer datasheet"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("flash", type=Path)
    parser.add_argument("--rom", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    data = args.flash.read_bytes()
    report = audit(data, args.rom.read_bytes())
    out = args.output.absolute()
    require(not out.is_symlink() and out.resolve().is_relative_to((ROOT / "private").resolve()),
            "Output must be a new ignored private directory")
    os.umask(0o077)
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    for segment in report["factory_segments"]:
        start, length = int(segment["body_offset"], 16), segment["body_bytes"]
        (out / (segment["name"] + ".bin")).write_bytes(data[start:start + length])
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print("PASS: four bounded factory headers and eight supplied-state ROM selector cases")
    print("No physical strap operation or recovery is established by this offline report.")


if __name__ == "__main__":
    main()
