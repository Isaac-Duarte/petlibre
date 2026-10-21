#!/usr/bin/env python3
"""Independent offline TX policy for the one pinned diagnostic's two directions.

Does not import the installer, recovery transport or UART implementation.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]
BASELINE_SHA = "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc"
CANDIDATE_SHA = "6f97391a7afa43b512987ac73eb7120a09d57450928f8f653ea05a6ff9aa8c97"
START, PAYLOAD_BYTES = 0x186000, 106496


def digest(data):
    return hashlib.sha256(data).hexdigest()


def expected(baseline, candidate, direction, *, include_full_reads=True):
    if len(baseline) != 0x800000 or digest(baseline) != BASELINE_SHA:
        raise ValueError("Pinned original backup required")
    if len(candidate) != PAYLOAD_BYTES + 32 or digest(candidate) != CANDIDATE_SHA:
        raise ValueError("Pinned diagnostic required")
    if direction not in ("install", "restore") or type(include_full_reads) is not bool:
        raise ValueError("Explicit direction and boolean snapshot profile required")
    payload = candidate[32:] if direction == "install" else baseline[START:START + PAYLOAD_BYTES]

    def read(offset, size):
        return b"\x20" + offset.to_bytes(3, "little") + (size // 4096).to_bytes(2, "little") + b"\x06" * (size // 1024 + 1)

    def packet(number, offset, data):
        body = struct.pack("<I", 0x08000000 + offset) + data
        return bytes([2, number, number ^ 255]) + body + bytes([sum(body) & 255])

    status = b"".join(bytes([0x21, op, 1]) for _ in range(2) for op in (5, 0x35, 0x15))
    full = read(0, 0x800000) if include_full_reads else b""
    parts = [full]
    for at in range(0, PAYLOAD_BYTES, 4096):
        offset, sector = START + at, payload[at:at + 4096]
        if at == 0:
            sector = b"\xff" * 8 + sector[8:]
        parts.extend([read(offset, 4096), status,
                      b"\x17" + offset.to_bytes(3, "little") + b"\x01\x00",
                      read(offset, 4096), status, b"\x07"])
        parts.extend(packet(i + 1, offset + 1024 * i, sector[1024 * i:1024 * (i + 1)]) for i in range(4))
        parts.extend([b"\x04", read(offset, 4096)])
    parts.extend([full, read(START, 4096), status, b"\x07", packet(1, START, payload[:1024]),
                  b"\x04", read(START, 4096), full])
    return b"".join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("direction", choices=("install", "restore"))
    for name in ("baseline", "candidate", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if not all(p.resolve().is_relative_to((ROOT / "private").resolve())
               for p in (args.baseline, args.candidate, args.output)):
        raise ValueError("Private inputs and new output required")
    wire = expected(args.baseline.read_bytes(), args.candidate.read_bytes(), args.direction)
    os.umask(0o077)
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    (args.output / "expected-serial-tx.bin").write_bytes(wire)
    report = {"direction": args.direction, "candidate_sha256": CANDIDATE_SHA,
              "baseline_sha256": BASELINE_SHA, "sector_erases": 26, "staging_packets": 104,
              "commit_packets": 1, "additional_commit_erases": 0, "full_reads": 3,
              "sector_reads": 80, "status_requests": 318,
              "tx_bytes": len(wire), "tx_sha256": digest(wire),
              "tool_sha256": digest(Path(__file__).read_bytes()),
              "limit": "No duplicate read frames modeled; extra ACKs require matching captured duplicate-frame evidence"}
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
