#!/usr/bin/env python3
"""Execute the real stock GET_CONFIG and compiled probe offline, never on hardware."""

import argparse
import importlib.util
import importlib.metadata
import json
from pathlib import Path
import struct

from unicorn import (Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS,
                     UC_HOOK_CODE, UC_PROT_READ, UC_PROT_EXEC)
from unicorn import arm_const as arm
from capstone import Cs, CS_ARCH_ARM, CS_MODE_THUMB, CS_MODE_LITTLE_ENDIAN
from capstone.arm import (ARM_INS_PUSH, ARM_INS_STM, ARM_INS_STR, ARM_INS_STRB,
                          ARM_INS_STRH, ARM_INS_STRD, ARM_OP_MEM)

if not __debug__:
    raise RuntimeError("Emulation checks require Python assertions; do not use -O")

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("c_probe", ROOT / "tools/prepare-c-probe.py")
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)
FLASH = 0x0E000000
RAM = 0x10000000
STACK = 0x100F0000
HEAP = 0x10080000
FIXTURE = 0x100D0000
EXIT = 0x0E000100
PRINT_RETURN = EXIT + 4


def proof(message_id):
    value = 2166136261
    for byte in message_id.encode("ascii"):
        value = ((value ^ byte) * 16777619) & 0xFFFFFFFF
    return f"c1-{value:08x}"


def verify_topic_layout(image):
    """Run the stock topic initializer and lookup with a synthetic device ID.

    Model only ROM memset and sprintf, checking the exact four-string format.
    This catches the former simulator assumption that GET_CONFIG uses system.
    """
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(FLASH, 0x200000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(FLASH, image[0x17020:])
    uc.mem_map(RAM, 0x100000)
    uc.mem_map(0x10100000, 0x20000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(0x100213EC, b"AF_TEST_ONLY\0")
    topics = []

    def string(pointer):
        data = bytes(uc.mem_read(pointer, 100)).split(b"\0", 1)
        assert len(data) == 2
        return data[0].decode("ascii")

    def returned(value):
        uc.reg_write(arm.UC_ARM_REG_R0, value)
        uc.reg_write(arm.UC_ARM_REG_PC, uc.reg_read(arm.UC_ARM_REG_LR))

    def instruction(_uc, address, _size, _data):
        if address == EXIT:
            uc.emu_stop()
        elif address == 0x10110EA0:
            destination, value, count = [uc.reg_read(r) for r in
                [arm.UC_ARM_REG_R0, arm.UC_ARM_REG_R1, arm.UC_ARM_REG_R2]]
            assert destination == 0x100214AC and value == 0 and count == 1500
            uc.mem_write(destination, b"\0" * count)
            returned(destination)
        elif address == 0x0E02B890:
            destination = uc.reg_read(arm.UC_ARM_REG_R0)
            assert destination == 0x100214AC + len(topics) * 100
            assert string(uc.reg_read(arm.UC_ARM_REG_R1)) == "dl/%s/%s/device/%s/%s"
            stack = uc.reg_read(arm.UC_ARM_REG_SP)
            suffixes = struct.unpack("<II", uc.mem_read(stack, 8))
            args = [string(uc.reg_read(reg)) for reg in [arm.UC_ARM_REG_R2, arm.UC_ARM_REG_R3]]
            args += [string(pointer) for pointer in suffixes]
            assert args[:2] == ["PLAF103", "AF_TEST_ONLY"]
            topic = "dl/%s/%s/device/%s/%s" % tuple(args)
            assert len(topic) < 100
            topics.append(topic)
            uc.mem_write(destination, topic.encode("ascii") + b"\0")
            returned(len(topic))
        else:
            assert (0x0E0122A0 <= address < 0x0E012426 or
                    0x0E012CE4 <= address < 0x0E012D5C), f"unexpected topic initialization {address:#x}"

    uc.hook_add(UC_HOOK_CODE, instruction)
    for entry, argument in [(0x0E0122A1, 0), (0x0E012CE5, 6)]:
        uc.reg_write(arm.UC_ARM_REG_SP, STACK)
        uc.reg_write(arm.UC_ARM_REG_LR, EXIT | 1)
        uc.reg_write(arm.UC_ARM_REG_R0, argument)
        uc.emu_start(entry, EXIT, timeout=1_000_000, count=2000)
        assert uc.reg_read(arm.UC_ARM_REG_PC) == EXIT
    assert len(topics) == 15
    assert topics[6].endswith("/config/post") and topics[7].endswith("/config/sub")
    assert uc.reg_read(arm.UC_ARM_REG_R0) == 0x10021704
    assert string(uc.reg_read(arm.UC_ARM_REG_R0)) == topics[6]
    return {"stock_initializer_and_lookup_executed": True,
            "get_config_topic_index": 6, "reply_suffix": "config/post", "query_suffix": "config/sub",
            "modeled": ["ROM memset", "stock sprintf"]}


def execute(image, message_id, failure=None):
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(FLASH, 0x200000, UC_PROT_READ | UC_PROT_EXEC)
    uc.mem_write(FLASH, image[0x17020:])
    uc.mem_map(RAM, 0x100000)
    uc.mem_write(0x10005000, image[0x135090:0x14FC70])
    uc.mem_write(FIXTURE, message_id.encode("ascii") + b"\0")
    uc.mem_write(FIXTURE + 256, b"00:11:22:33:44:55\0")
    preserved = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(4, 12)]
    for i, reg in enumerate(preserved):
        uc.reg_write(reg, 0xA4000000 + i * 256)
    uc.reg_write(arm.UC_ARM_REG_SP, STACK)
    uc.reg_write(arm.UC_ARM_REG_LR, EXIT | 1)
    captured = []
    live = {}
    guards = []
    bump = HEAP
    fail_next = False
    failed = False
    entered = False
    finished = False
    minimum_sp = STACK
    instructions = 0
    probe_writes = 0
    probe_key_writes = 0
    node_return = None
    key_return = None
    owned_node = None
    owned_key = None
    publication_return = None
    fail_after_allocations = None
    decoder = Cs(CS_ARCH_ARM, CS_MODE_THUMB | CS_MODE_LITTLE_ENDIAN)
    decoder.detail = True

    def word(address):
        return struct.unpack("<I", uc.mem_read(address, 4))[0]

    def string(address, limit=128):
        assert address != 0, "unexpected null string"
        value = bytearray()
        for offset in range(limit):
            byte = uc.mem_read(address + offset, 1)[0]
            if byte == 0:
                return value.decode("ascii")
            value.append(byte)
        raise AssertionError("unterminated emulated string")

    def returned(value):
        uc.reg_write(arm.UC_ARM_REG_R0, value)
        uc.reg_write(arm.UC_ARM_REG_PC, uc.reg_read(arm.UC_ARM_REG_LR))

    def check_probe_store(address, size):
        nonlocal probe_writes, probe_key_writes
        instruction = next(decoder.disasm(bytes(uc.mem_read(address, size)), address))
        aliases = {"sb": "r9", "sl": "r10", "fp": "r11", "ip": "r12"}

        def register(identifier):
            name = instruction.reg_name(identifier)
            return uc.reg_read(getattr(arm, "UC_ARM_REG_" + aliases.get(name, name).upper()))

        if instruction.id == ARM_INS_PUSH:
            count = 4 * len(instruction.operands)
            destination = uc.reg_read(arm.UC_ARM_REG_SP) - count
        elif instruction.id == ARM_INS_STM:
            count = 4 * (len(instruction.operands) - 1)
            destination = register(instruction.operands[0].reg)
        elif instruction.id in [ARM_INS_STR, ARM_INS_STRB, ARM_INS_STRH, ARM_INS_STRD]:
            memory = next(op.mem for op in instruction.operands if op.type == ARM_OP_MEM)
            assert not memory.index, "unreviewed indexed store"
            # A pre-indexed SP store with writeback is a one-register push (the
            # Rust probe saves r11 this way); its destination is still bounded below.
            assert not instruction.writeback or (
                instruction.reg_name(memory.base) == "sp" and not instruction.post_index
                and memory.disp < 0), "unreviewed writeback store"
            destination = register(memory.base) + memory.disp
            count = {ARM_INS_STR: 4, ARM_INS_STRB: 1, ARM_INS_STRH: 2, ARM_INS_STRD: 8}[instruction.id]
        else:
            assert not instruction.mnemonic.startswith("st"), "unreviewed probe store instruction"
            return
        if STACK - 4096 <= destination and destination + count <= STACK:
            probe_writes += 1
            return
        # The only direct heap store initializes the newly allocated diagnostic
        # node's key before the stock list function takes ownership. Its node and
        # string must have just been returned to this probe by the stock APIs.
        assert (owned_node and owned_key and instruction.id == ARM_INS_STR and count == 4
                and destination == owned_node + 32 and live.get(owned_node) == 40
                and owned_key in live and word(owned_node + 32) == 0
                and all(word(owned_node + offset) == 0 for offset in [0, 4, 8])
                and word(owned_node + 12) == 4
                and live.get(word(owned_node + 16)) == 12
                and string(word(owned_node + 16)).startswith("c1-")
                and string(owned_key) == "localReProbe"
                and register(instruction.operands[0].reg) == owned_key
                and probe_key_writes == 0), "probe wrote outside its stack or owned key field"
        probe_key_writes += 1

    def on_code(_uc, address, _size, _data):
        nonlocal bump, fail_next, failed, entered, finished, minimum_sp, instructions
        nonlocal publication_return, fail_after_allocations
        nonlocal node_return, key_return, owned_node, owned_key
        instructions += 1
        minimum_sp = min(minimum_sp, uc.reg_read(arm.UC_ARM_REG_SP))
        if address == node_return:
            owned_node = uc.reg_read(arm.UC_ARM_REG_R0)
            node_return = None
        if address == key_return:
            owned_key = uc.reg_read(arm.UC_ARM_REG_R0)
            key_return = None
        if address == EXIT:
            finished = True
            uc.emu_stop()
            return
        if (0x0E0C800C <= address < 0x0E0C8280 or
                0x0E0C8E2A <= address < 0x0E0C8EB4 or
                0x0E0C9074 <= address < 0x0E0C9130 or
                0x0E0CA070 <= address < 0x0E0CA100):
            return  # Execute the actual stock libc, including its register effects.
        if address == PATCH.SLOT_ADDRESS:
            entered = True
        if PATCH.SLOT_ADDRESS <= address < PATCH.SLOT_ADDRESS + PATCH.SLOT_BYTES:
            check_probe_store(address, _size)
        if address == PRINT_RETURN:
            pointer = uc.reg_read(arm.UC_ARM_REG_R0)
            serialized = string(pointer, 2048)
            try:
                captured.append(json.loads(serialized))
            except ValueError as error:
                raise AssertionError(f"Malformed stock serialization ({failure}): {serialized!r}") from error
            assert pointer in live
            del live[pointer]  # Publication owns and frees the serialized buffer.
            uc.reg_write(arm.UC_ARM_REG_LR, publication_return)
            publication_return = None
            returned(0)
            return
        if address in [value & ~1 for value in PATCH.APIS.values()]:
            assert uc.reg_read(arm.UC_ARM_REG_SP) % 8 == 0, "unaligned stock API call"
        if address == 0x1000AB80:  # Stock allocator hook, heap ownership modeled.
            size = uc.reg_read(arm.UC_ARM_REG_R0)
            assert 0 < size <= 4096
            if fail_next:
                fail_next = False
                failed = True
                returned(0)
                return
            if fail_after_allocations is not None:
                if fail_after_allocations == 0:
                    fail_after_allocations = None
                    failed = True
                    returned(0)
                    return
                fail_after_allocations -= 1
            pointer = (bump + 23) & ~7
            assert pointer + size + 8 < HEAP + 0x20000
            uc.mem_write(pointer - 8, b"\xa5" * (size + 16))
            guards.append((pointer, size))
            live[pointer] = size
            bump = pointer + size + 8
            returned(pointer)
        elif address == 0x1000AC38:  # Stock free hook.
            pointer = uc.reg_read(arm.UC_ARM_REG_R0)
            assert pointer in live, "unknown or double free"
            del live[pointer]
            returned(0)
        elif address == 0x0E014044:  # Stock ID generator: varying deterministic fixture.
            returned(FIXTURE)
        elif address == 0x0E009024:  # Stock MAC getter: no hardware access.
            returned(FIXTURE + 256)
        elif address == 0x0E012CE4:  # Stock topic lookup.
            assert uc.reg_read(arm.UC_ARM_REG_R0) == 6
            returned(0x1234)
        elif address == 0x0E0133C8:  # Observe publication; never opens a socket.
            assert uc.reg_read(arm.UC_ARM_REG_R0) == 0x1234
            assert publication_return is None
            publication_return = uc.reg_read(arm.UC_ARM_REG_LR)
            uc.reg_write(arm.UC_ARM_REG_R0, uc.reg_read(arm.UC_ARM_REG_R1))
            uc.reg_write(arm.UC_ARM_REG_LR, PRINT_RETURN | 1)
            uc.reg_write(arm.UC_ARM_REG_PC, 0x0E00BC19)  # Stock JSON print.
        elif address == 0x0E00BCFA:
            value = string(uc.reg_read(arm.UC_ARM_REG_R0))
            if value.startswith("c1-"):
                node_return = uc.reg_read(arm.UC_ARM_REG_LR) & ~1
                assert PATCH.SLOT_ADDRESS <= node_return < PATCH.SLOT_ADDRESS + PATCH.SLOT_BYTES
            if not failed and ((failure == "probe_allocation" and value.startswith("c1-")) or
                    (failure == "message_allocation" and value == message_id)):
                fail_next = True
            elif not failed and failure == "probe_value_allocation" and value.startswith("c1-"):
                fail_after_allocations = 1
        elif address == 0x0E00B144:
            if string(uc.reg_read(arm.UC_ARM_REG_R0)) == "localReProbe":
                key_return = uc.reg_read(arm.UC_ARM_REG_LR) & ~1
                assert PATCH.SLOT_ADDRESS <= key_return < PATCH.SLOT_ADDRESS + PATCH.SLOT_BYTES
                if failure == "probe_key_allocation" and not failed:
                    fail_next = True
        else:
            allowed = (0x0E0141D0 <= address < 0x0E014260 or
                       0x0E00AF00 <= address < 0x0E00BE00 or
                       0x0E0C7D6E <= address < 0x0E0C7D82 or
                       PATCH.SLOT_ADDRESS <= address < PATCH.SLOT_ADDRESS + PATCH.SLOT_BYTES)
            assert allowed, f"unexpected execution address {address:#x}"

    uc.hook_add(UC_HOOK_CODE, on_code)
    # A normal memory-write hook makes Unicorn 2.1.4 misexecute Thumb IT blocks
    # in untouched stock memcpy/JSON formatting. Audit the probe's effective
    # store addresses before each instruction instead; retain heap canaries and
    # read/execute-only flash. Stock memory writes are not instrumented globally.
    uc.emu_start(0x0E0141D1, EXIT, timeout=2_000_000, count=20_000)
    finished = finished or uc.reg_read(arm.UC_ARM_REG_PC) == EXIT
    assert finished, "handler did not return within its instruction/time budget"
    assert len(captured) == 1 and not live, "missing publication or JSON allocation leak"
    for pointer, size in guards:
        assert bytes(uc.mem_read(pointer - 8, 8)) == b"\xa5" * 8
        assert bytes(uc.mem_read(pointer + size, 8)) == b"\xa5" * 8
    for i, reg in enumerate(preserved):
        assert uc.reg_read(reg) == 0xA4000000 + i * 256, "callee-saved register clobbered"
    assert uc.reg_read(arm.UC_ARM_REG_SP) == STACK, "stack not restored"
    if failure:
        assert failed, "allocation failure was not exercised"
    return {"fields": captured[0], "probe_entered": entered, "probe_stack_writes": probe_writes,
            "probe_owned_key_writes": probe_key_writes,
            "stack_bytes_including_stock_calls": STACK - minimum_sp,
            "instructions": instructions, "json_allocations": len(guards)}


def verify_reply(baseline, result, message_id):
    expected = dict(baseline["fields"], softwareVersion="1.1.17")
    if 0 < len(message_id) < 48:
        expected["localReProbe"] = proof(message_id)
    assert result["fields"] == expected, "probe changed a reply field or returned an incorrect proof"
    assert result["probe_entered"], "patched instruction did not execute the probe"


def verify(directory):
    original = PATCH.STOCK.read_bytes()
    candidate = (directory / "c-probe-1.1.17.bin").read_bytes()
    code = (directory / "probe.bin").read_bytes()
    manifest = json.loads((directory / "manifest.json").read_text())
    assert PATCH.digest(candidate) == manifest["candidate_sha256"]
    assert PATCH.digest(code) == manifest["probe_sha256"]
    PATCH.verify_patch(original, candidate, code)
    cases = []
    for message_id in ["probe-0001", "probe-0002", "x" * 47, "", "x" * 48]:
        baseline = execute(original, message_id)
        result = execute(candidate, message_id)
        verify_reply(baseline, result, message_id)
        cases.append({"message_id_bytes": len(message_id), **result})
    for failure in ["probe_allocation", "message_allocation",
                    "probe_value_allocation", "probe_key_allocation"]:
        baseline = execute(original, "probe-0001", failure if failure == "message_allocation" else None)
        result = execute(candidate, "probe-0001", failure)
        assert result["fields"] == dict(baseline["fields"], softwareVersion="1.1.17")
        cases.append({"failure": failure, **result})
    report = {"candidate_sha256": manifest["candidate_sha256"], "cases": cases,
              "topic_contract": verify_topic_layout(candidate),
              "emulator_sha256": PATCH.digest(Path(__file__).read_bytes()),
              "dependencies": {name: importlib.metadata.version(name) for name in ["unicorn", "capstone"]},
              "stock_json_construction_serialization_and_destruction_executed": True,
              "modeled": ["allocator/free", "ID generation", "MAC getter", "topic lookup",
                          "publication (executes stock JSON print, omits timestamp/network)"],
              "hardware_access": False, "actual_flash_mapping_or_mpu_proven": False}
    (directory / "emulation.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    print(json.dumps(verify(args.directory), indent=2))
