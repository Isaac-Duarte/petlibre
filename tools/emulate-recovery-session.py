#!/usr/bin/env python3
"""Exercise actual vendor-helper erase/program/read transport on synthetic NOR.

No serial import or physical device access. ROM UART/flash APIs are modeled;
the helper's command parser, packet checks and program wrapper execute as ARM.
"""
import argparse
from collections import deque
from datetime import datetime, timezone
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
from unicorn import UC_PROT_READ, UC_PROT_EXEC
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin"
HELPER_SHA = "9307121385cb390dfd2da64da2c6c515f17b5a9556b3d04021487c9b9f220b55"
TARGET, SECTOR, CAPACITY = 0x186000, 4096, 0x800000
SPEC = importlib.util.spec_from_file_location("uart_reader", ROOT / "tools/uart-flash-read.py")
READER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(READER)


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


class EmulatedHelperPort:
    """Serial-shaped in-memory peer; contains no OS port-opening code."""
    def __init__(self, helper, snapshot=None, allowed_sectors=None, status_registers=None):
        require(len(helper) == 4688 and digest(helper) == HELPER_SHA, "Pinned helper required")
        self.uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
        self.uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
        self.uc.mem_map(0, 0x10000)
        self.uc.mem_map(0x80000, 0x10000)
        self.uc.mem_write(0x82000, helper)
        self.uc.mem_map(0x08000000, CAPACITY, UC_PROT_READ | UC_PROT_EXEC)
        self.before = bytes(snapshot) if snapshot is not None else b"\xa5" * CAPACITY
        require(len(self.before) == CAPACITY, "Synthetic snapshot must be 8 MiB")
        self.allowed_sectors = frozenset(allowed_sectors if allowed_sectors is not None else [TARGET])
        require(self.allowed_sectors and all(type(s) is int and 0x186000 <= s < 0x2D6000
                                            and s % SECTOR == 0 for s in self.allowed_sectors),
                "Synthetic erase/program scope must stay in stock bank 1")
        self.uc.mem_write(0x08000000, self.before)
        self.uc.mem_map(0x48000000, 0x1000)
        self.uc.mem_map(0xE000E000, 0x2000)
        self.uc.reg_write(arm.UC_ARM_REG_SP, 0x8FF00)
        self.incoming, self.outgoing = deque(), bytearray()
        self.host_tx = bytearray()
        require(status_registers is None or (len(status_registers) == 3
                and all(type(v) is int and 0 <= v <= 255 for v in status_registers)), "Three synthetic status bytes required")
        self.status_registers = None if status_registers is None else dict(zip((0x05, 0x35, 0x15), status_registers))
        self.calls = []
        self.instructions = self.empty_polls = 0
        self.started = False
        self.uc.hook_add(UC_HOOK_CODE, self._step)

    def _return(self, value=0):
        self.uc.reg_write(arm.UC_ARM_REG_R0, value)
        self.uc.reg_write(arm.UC_ARM_REG_PC, self.uc.reg_read(arm.UC_ARM_REG_LR))

    def _step(self, _uc, pc, _size, _data):
        self.instructions += 1
        if 0x82020 <= pc < 0x83030:
            return
        args = [self.uc.reg_read(getattr(arm, f"UC_ARM_REG_R{i}")) for i in range(3)]
        if pc == 0x6220:
            if not self.incoming and self.outgoing:
                self.uc.emu_stop()  # Resume this call after host consumes output/sends input.
                return
            self.empty_polls = self.empty_polls + 1 if not self.incoming else 0
            require(self.empty_polls < 100, "Helper waiting without an expected response")
            self._return(int(bool(self.incoming)))
        elif pc == 0x6250:
            require(self.incoming, "UART getc without supplied host input")
            self._return(self.incoming.popleft())
        elif pc == 0x6274:
            self.outgoing.append(args[0] & 255)
            self._return(1)
        elif pc == 0x628C:
            pointer, length = args[:2]
            require(0x80000 <= pointer and pointer + length <= 0x90000 and length <= 1032,
                    "UART transmit escaped helper RAM")
            self.outgoing.extend(self.uc.mem_read(pointer, length))
            self._return(length)
        elif pc == 0x1D1C:
            opcode, length, pointer = args
            require(self.status_registers is not None and opcode in self.status_registers and length == 1
                    and 0x80000 <= pointer < 0x90000, "Unexpected status-read command or buffer")
            self.uc.mem_write(pointer, bytes([self.status_registers[opcode]]))
            self.calls.append({"name": "read_status", "opcode": opcode, "bytes": 1})
            self._return()
        elif pc == 0x6238:
            require(self.status_registers is not None, "Unexpected UART-writable polling")
            self._return(1)
        elif pc == 0x2146:
            require(args[0] == 2 and args[1] in self.allowed_sectors, "Erase escapes allowed application sectors")
            self.calls.append({"name": "erase", "offset": args[1], "bytes": SECTOR})
            self.uc.mem_write(0x08000000 + args[1], b"\xff" * SECTOR)
            self._return()
        elif pc == 0x2048:
            offset, length, pointer = args
            require(length == 8 and offset // SECTOR * SECTOR in self.allowed_sectors
                    and offset % SECTOR + length <= SECTOR, "Program escapes allowed application sectors")
            require(0x80000 <= pointer and pointer + length <= 0x90000, "Program buffer outside helper RAM")
            existing = bytes(self.uc.mem_read(0x08000000 + offset, length))
            data = bytes(self.uc.mem_read(pointer, length))
            self.uc.mem_write(0x08000000 + offset, bytes(a & b for a, b in zip(existing, data)))
            self.calls.append({"name": "program", "offset": offset, "bytes": length})
            self._return()
        elif pc == 0x6590:
            destination, source, length = args
            require(0 <= length <= 4096 and 0x80000 <= destination and destination + length <= 0x90000
                    and 0x80000 <= source and source + length <= 0x90000, "Memcpy escaped helper RAM")
            self.uc.mem_write(destination, bytes(self.uc.mem_read(source, length)))
            self._return(destination)
        elif pc == 0x66AC:
            destination, value, length = args
            require(0 <= length <= 4096 and 0x80000 <= destination and destination + length <= 0x90000,
                    "Memset escaped helper RAM")
            self.uc.mem_write(destination, bytes([value & 255]) * length)
            self._return(destination)
        elif pc == 0x61A0:
            require(args == [2, 0, 115200], "Unexpected helper UART initialization")
            self.uc.mem_write(0x80660, struct.pack("<I", 0x40008000))
            self._return()
        elif pc == 0x3594:
            require(args[0] == 0, "Unexpected OTF setup")
            self.uc.mem_write(0x48000600, b"\0" * 4)
            self._return()
        elif pc == 0x1434:
            require(args[0] == 0, "Unexpected backup register")
            self._return(0)
        elif pc in (0x0474, 0x14C2, 0x1504, 0x40B0, 0x43D4):
            self._return()  # Diagnostic, delay, UART baud/busy models only.
        else:
            raise ValueError(f"Unexpected helper destination (unreviewed flash/register operation forbidden): {pc:#x}")

    def _drive(self):
        pc = self.uc.reg_read(arm.UC_ARM_REG_PC) if self.started else 0x82020
        self.started = True
        self.empty_polls = 0
        self.uc.emu_start(pc | 1, 0x83030, count=500000, timeout=3_000_000)
        require(self.outgoing, "Helper produced no bounded response")

    @property
    def in_waiting(self):
        return len(self.outgoing)

    def read(self, count):
        if not self.outgoing:
            self._drive()
        result = bytes(self.outgoing[:count])
        del self.outgoing[:count]
        return result

    def write(self, data):
        self.incoming.extend(data)
        self.host_tx.extend(data)
        return len(data)

    def flush(self):
        pass

    def snapshot(self):
        return bytes(self.uc.mem_read(0x08000000, CAPACITY))


def command_ready(port):
    READER.wait_naks(port)
    pending = port.in_waiting
    if pending:
        require(port.read(pending) == b"\x15" * pending, "Unexpected buffered command output")


def program_frame(number, offset, payload):
    require(1 <= number <= 4 and len(payload) == 1024, "Exactly four full 1 KiB packets required")
    require(TARGET <= offset and offset + len(payload) <= TARGET + SECTOR, "Packet outside allowed sector")
    body = struct.pack("<I", 0x08000000 + offset) + payload
    return bytes([2, number, number ^ 255]) + body + bytes([sum(body) & 255])


def execute():
    port = EmulatedHelperPort(HELPER.read_bytes())
    payload = bytes((i * 7 + 3) & 255 for i in range(SECTOR))
    command_ready(port)
    port.write(b"\x17" + TARGET.to_bytes(3, "little") + b"\x01\x00")
    READER.expect(port, READER.ACK)
    command_ready(port)
    port.write(b"\x07")
    READER.expect(port, READER.ACK)
    for number in range(1, 5):
        port.write(program_frame(number, TARGET + (number - 1) * 1024,
                                 payload[(number - 1) * 1024:number * 1024]))
        READER.expect(port, READER.ACK)
    port.write(b"\x04")
    READER.expect(port, READER.ACK)
    output = io.BytesIO()
    read = READER.receive_flash(port, TARGET, SECTOR, output)
    require(output.getvalue() == payload, "Readback differs from sector payload")
    snapshot = port.snapshot()
    require(snapshot[:TARGET] == port.before[:TARGET] and snapshot[TARGET + SECTOR:] == port.before[TARGET + SECTOR:],
            "Synthetic flash changed outside the selected sector")
    return {"created_utc": datetime.now(timezone.utc).isoformat(), "helper_sha256": HELPER_SHA,
            "explicit_erase_command": "17 00 60 18 01 00", "target_offset": hex(TARGET),
            "program_packets": 4, "readback_packets": read["packets"], "readback_sha256": digest(output.getvalue()),
            "erase_calls": sum(c["name"] == "erase" for c in port.calls),
            "eight_byte_program_calls": sum(c["name"] == "program" for c in port.calls),
            "outside_target_unchanged": True, "instructions": port.instructions,
            "limits": ["ROM flash/UART APIs and NOR/controller behavior modeled",
                       "No real serial device opened; protection/encryption and write acceptance unverified",
                       "ROM upload phase covered separately; this starts from the pinned helper in modeled RAM"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    require(args.report.resolve().is_relative_to((ROOT / "private").resolve()), "Report must be private")
    report = execute()
    args.report.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with args.report.open("x") as handle:
        handle.write(json.dumps(report, indent=2) + "\n")
    print("Offline erase/program/readback session passed; no physical device I/O")


if __name__ == "__main__":
    main()
