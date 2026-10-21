#!/usr/bin/env python3
"""GD25Q64E status decoding and injected-transport reads; no port-opening code.

CLI decodes supplied bytes offline. Reading helpers accept only 05/35/15 status
commands with length one; no status write, WREN, erase, resume or reset exists.
"""
import argparse
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CAPACITY = 0x800000


def require(value, message):
    if not value:
        raise ValueError(message)


def decode(sr1, sr2, sr3):
    require(all(type(value) is int and 0 <= value <= 255 for value in (sr1, sr2, sr3)),
            "Three integer status bytes required")
    bp = (sr1 >> 2) & 31
    level = bp & 7
    small, lower, complement = bool(bp & 16), bool(bp & 8), bool(sr2 & 64)
    if level == 0:
        protected = None
    elif level == 7:
        protected = (0, CAPACITY)
    else:
        length = (4096 << min(level - 1, 3)) if small else (1 << (16 + level))
        protected = (0, length) if lower else (CAPACITY - length, CAPACITY)
    if complement:
        if protected is None:
            protected = (0, CAPACITY)
        elif protected == (0, CAPACITY):
            protected = None
        elif protected[0] == 0:
            protected = (protected[1], CAPACITY)
        else:
            protected = (0, protected[0])
    return {"part": "GD25Q64E", "sr1": sr1, "sr2": sr2, "sr3": sr3,
            "busy": bool(sr1 & 1), "write_enable_latch": bool(sr1 & 2),
            "bp4_to_bp0": bp, "complement": complement,
            "erase_suspended": bool(sr2 & 128), "program_suspended": bool(sr2 & 4),
            "quad_enable": bool(sr2 & 2), "srp1_srp0": ((sr2 & 1) << 1) | (sr1 >> 7),
            "security_register_locks": [bool(sr2 & (1 << bit)) for bit in (3, 4, 5)],
            "driver_strength_field": (sr3 >> 5) & 3, "dummy_configuration": bool(sr3 & 1),
            "sr3_reserved_bits_raw": sr3 & 0x9E,
            "protected_ranges": [] if protected is None else [{"start": protected[0], "end_exclusive": protected[1]}],
            "scope": "Manufacturer BP/CMP and status fields only; physical write permission not established"}


def assess_range(sr1, sr2, sr3, start, end):
    require(type(start) is int and type(end) is int and 0 <= start < end <= CAPACITY,
            "Range must be within the 8 MiB main array")
    decoded = decode(sr1, sr2, sr3)
    overlaps = [{"start": max(start, area["start"]), "end_exclusive": min(end, area["end_exclusive"])}
                for area in decoded["protected_ranges"] if start < area["end_exclusive"] and area["start"] < end]
    reasons = []
    if decoded["busy"]:
        reasons.append("flash_busy")
    if decoded["erase_suspended"] or decoded["program_suspended"]:
        reasons.append("erase_or_program_suspended")
    if overlaps:
        reasons.append("software_protected_range")
    return {"range": {"start": start, "end_exclusive": end}, "protected_overlap": overlaps,
            "software_preconditions_met": not reasons, "blocking_reasons": reasons,
            "physical_write_permission_established": False, "decoded": decoded}


def read_status_registers(port):
    """Read only three whitelisted one-byte statuses from an existing helper.

    The caller establishes helper identity/session and captures evidence. This
    function neither uploads a helper nor opens a port or changes baud/straps.
    """
    spec = importlib.util.spec_from_file_location("uart_reader", ROOT / "tools/uart-flash-read.py")
    reader = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(reader)
    values = []
    for opcode in (0x05, 0x35, 0x15):
        reader.wait_naks(port)
        pending = port.in_waiting
        if pending:
            require(port.read(pending) == b"\x15" * pending, "Unexpected buffered helper output")
        reader.send(port, bytes([0x21, opcode, 1]))
        reader.expect(port, 0x21)
        values.append(reader.exact_read(port, 1)[0])
    return tuple(values)


def number(value):
    return int(value, 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sr1", required=True, type=number)
    parser.add_argument("--sr2", required=True, type=number)
    parser.add_argument("--sr3", required=True, type=number)
    parser.add_argument("--start", type=number, default=0x186000)
    parser.add_argument("--end", type=number, default=0x2D6000)
    args = parser.parse_args()
    try:
        result = assess_range(args.sr1, args.sr2, args.sr3, args.start, args.end)
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
