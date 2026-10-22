#!/usr/bin/env python3
"""Run the pinned vendor UART helper against synthetic flash and modeled UART; no device I/O."""

import argparse
from collections import Counter, deque
from datetime import datetime, timezone
import hashlib
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import struct

from unicorn import Uc, UcError, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE, UC_PROT_READ, UC_PROT_EXEC
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin"
HELPER_SHA256 = "9307121385cb390dfd2da64da2c6c515f17b5a9556b3d04021487c9b9f220b55"
SPEC = importlib.util.spec_from_file_location("probe_patch", ROOT / "tools/emulate/prepare-c-probe.py")
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)
ROM_CALLS = {
    0x0474: "DiagPrintf", 0x14C2: "DelayUs", 0x1504: "DelayMs", 0x1434: "BKUP_Read",
    0x2E9C: "GPIO_Init", 0x3594: "RSIP_OTF_Cmd", 0x40B0: "UART_SetBaud",
    0x43D4: "UART_WaitBusy", 0x61A0: "xmodem_uart_port_init", 0x61D4: "xmodem_uart_port_deinit",
    0x6220: "xmodem_uart_readable", 0x6250: "xmodem_uart_getc", 0x6274: "xmodem_uart_putc",
    0x628C: "xmodem_uart_putdata", 0x6590: "_memcpy", 0x66AC: "_memset",
}
FORBIDDEN_APIS = {0x2048: "FLASH_TxData12B", 0x2128: "FLASH_SetStatus",
                  0x2146: "FLASH_Erase", 0x2204: "FLASH_SetStatusBits"}

if not __debug__:
    raise RuntimeError("Emulation checks require assertions; do not use Python -O")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def execute(helper, rom_flag=0, backup_flag=0, nack_first=False):
    """A single 4 KiB read; modeled ROM calls intentionally do not access hardware."""
    assert rom_flag in [0, 1] and backup_flag in [0, 0x400]
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(0, 0x10000)
    uc.mem_map(0x80000, 0x10000)
    uc.mem_write(0x82000, helper)
    uc.mem_write(0x1298, struct.pack("<I", rom_flag))
    uc.mem_map(0x08000000, 0x4000, UC_PROT_READ | UC_PROT_EXEC)
    flash = bytes((i * 7 + i // 256) & 255 for i in range(0x4000))
    uc.mem_write(0x08000000, flash)
    uc.mem_map(0x48000000, 0x1000)
    uc.mem_write(0x480003F8, struct.pack("<I", 0x12345678))
    uc.reg_write(arm.UC_ARM_REG_SP, 0x8FF00)
    uc.reg_write(arm.UC_ARM_REG_R1, 0x08000000)  # Also guards the direct-write mutation test.
    incoming = deque()
    accepted = []
    attempts = []
    calls = Counter()
    effects = []
    started = ended = finished = rejected = False
    instructions = 0

    def returned(value=0):
        uc.reg_write(arm.UC_ARM_REG_R0, value)
        uc.reg_write(arm.UC_ARM_REG_PC, uc.reg_read(arm.UC_ARM_REG_LR))

    def ram_range(address, count):
        assert 0 < count <= 4096 and 0x80000 <= address and address + count <= 0x90000, "ROM memory operation escaped fixture RAM"

    def on_code(_uc, address, _size, _data):
        nonlocal started, ended, finished, rejected, instructions
        instructions += 1
        if 0x82020 <= address < 0x83030:
            return  # Execute actual helper instructions, not decompiled pseudocode.
        args = [uc.reg_read(register) for register in [arm.UC_ARM_REG_R0, arm.UC_ARM_REG_R1, arm.UC_ARM_REG_R2]]
        assert address not in FORBIDDEN_APIS, "helper reached forbidden flash mutation API: " + FORBIDDEN_APIS.get(address, "unknown")
        assert address in ROM_CALLS, f"unexpected helper execution {address:#x}"
        calls[ROM_CALLS[address]] += 1
        if address == 0x6274:
            value = args[0] & 255
            if value == 0x15 and not started:
                started = True
                # Command 0x20, 24-bit offset 0x1000, one 4 KiB block.
                incoming.extend(bytes([0x20, 0, 0x10, 0, 1, 0]))
            elif value == 4:
                assert len(accepted) == 4 and not incoming
                ended = True
                incoming.append(6)
            elif value == 0x15 and ended:
                assert not incoming, "final ACK was not consumed"
                finished = True
                uc.emu_stop()
                return
            else:
                raise AssertionError(f"unexpected UART control output {value:#x}")
            returned(1)
        elif address == 0x628C:
            ram_range(args[0], args[1])
            packet = bytes(uc.mem_read(args[0], args[1]))
            assert len(packet) == 1028 and packet[0] == 2, "wrong STX packet size/type"
            assert packet[1] == len(accepted) + 1 and packet[1] ^ packet[2] == 255
            assert sum(packet[3:-1]) & 255 == packet[-1], "wrong additive packet checksum"
            attempts.append(packet)
            if nack_first and not rejected:
                rejected = True
                incoming.append(0x15)
            else:
                if nack_first and not accepted:
                    assert packet == attempts[0], "NAK did not resend the identical packet"
                accepted.append(packet)
                incoming.append(6)
            returned(len(packet))
        elif address == 0x6220:
            returned(int(bool(incoming)))
        elif address == 0x6250:
            assert incoming, "helper read without available UART data"
            returned(incoming.popleft())
        elif address == 0x6590:
            ram_range(args[0], args[2])
            ram_range(args[1], args[2])
            uc.mem_write(args[0], bytes(uc.mem_read(args[1], args[2])))
            returned(args[0])
        elif address == 0x66AC:
            ram_range(args[0], args[2])
            uc.mem_write(args[0], bytes([args[1] & 255]) * args[2])
            returned(args[0])
        elif address == 0x3594:
            assert args[0] == 0, "helper did not disable OTF as expected"
            effects.append({"name": "RSIP_OTF_Cmd", "enable": False})
            # Candidate ROM writes 0 to RSIP control at 0x48000600.
            uc.mem_write(0x48000600, struct.pack("<I", 0))
            returned()
        elif address == 0x61A0:
            assert args == [2, 0, 115200]
            effects.append({"name": "xmodem_uart_port_init", "port": 2, "baud": 115200})
            # Model its ROM-owned pointer, not the physical UART registers.
            uc.mem_write(0x80660, struct.pack("<I", 0x40008000))
            returned()
        elif address == 0x1434:
            assert args[0] == 0
            returned(backup_flag)
        elif address == 0x2E9C:
            assert backup_flag == 0x400
            ram_range(args[0], 24)
            effects.append({"name": "GPIO_Init", "fixture_branch": "BKUP_Read(0) bit 10 set"})
            returned()
        else:
            returned()  # Named UART setup, delay or diagnostic model.

    uc.hook_add(UC_HOOK_CODE, on_code)
    uc.emu_start(0x82021, 0x83030, timeout=3_000_000, count=500_000)
    assert finished and ended, "helper did not finish the bounded read"
    received = b"".join(packet[3:-1] for packet in accepted)
    assert len(received) == 4096 and received == flash[0x1000:0x2000]
    assert bytes(uc.mem_read(0x08000000, len(flash))) == flash, "synthetic flash changed"
    assert calls["GPIO_Init"] == int(bool(backup_flag))
    assert struct.unpack("<I", uc.mem_read(0x48000054, 4))[0] == (2 if rom_flag else 0)
    return {"rom_word_0x1298_fixture": rom_flag, "backup_register_fixture": backup_flag,
            "nack_first_packet_fixture": nack_first, "accepted_packets": len(accepted),
            "packet_attempts": len(attempts), "received_bytes": len(received), "received_sha256": digest(received),
            "synthetic_flash_unchanged": True, "instructions": instructions,
            "modeled_rom_calls": dict(sorted(calls.items())), "observed_setup_requests": effects}


def audit():
    helper = HELPER.read_bytes()
    assert len(helper) == 4688 and digest(helper) == HELPER_SHA256, "not the pinned vendor helper"
    return {"created_at": datetime.now(timezone.utc).isoformat(), "helper_sha256": HELPER_SHA256,
            "cases": [execute(helper, flag, backup, retry) for flag, backup, retry in
                      [(0, 0, False), (1, 0, False), (0, 0x400, False), (1, 0x400, True)]],
            "tool_sha256": digest(Path(__file__).read_bytes()), "unicorn_version": importlib.metadata.version("unicorn"),
            "rom_call_address_source": "pinned SDK KM0 acut target_rom.axf symbols; ROM calls are modeled, not executed",
            "limits": {"device_compatibility_verified": False, "real_rom_side_effects_verified": False,
                       "real_flash_encryption_state_verified": False, "physical_read_verified": False,
                       "full_capacity_read_verified": False, "serial_port_opened": False},
            "conclusion": "Modeled startup and 4 KiB read reach no explicit flash program/erase/status API; emitted bytes match synthetic flash. Actual ROM startup/read safety remains unverified."}


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
        print(f"Synthetic UART read checks passed; no device I/O. Report: {path}")
    except (OSError, ValueError, AssertionError, UcError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
