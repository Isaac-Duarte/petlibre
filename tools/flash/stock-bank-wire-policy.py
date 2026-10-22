#!/usr/bin/env python3
"""Construct an offline expected TX transcript for the stock bank-1 acceptance test.

Independent of the mutation/transport implementation. Never opens serial or
connects to a device. Raw original payloads and generated transcripts stay private.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[2]
BASELINE_SHA = "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc"
START, END, SECTOR = 0x186000, 0x2D6000, 4096


def digest(data):
    return hashlib.sha256(data).hexdigest()


def expected(baseline, *, include_full_reads=True):
    if len(baseline) != 0x800000 or digest(baseline) != BASELINE_SHA:
        raise ValueError("Pinned original full backup required")
    if type(include_full_reads) is not bool:
        raise ValueError("Explicit boolean full-read selection required")

    def read(offset, size):
        return b"\x20" + offset.to_bytes(3, "little") + (size // 4096).to_bytes(2, "little") + b"\x06" * (size // 1024 + 1)

    def packet(number, offset, data):
        body = struct.pack("<I", 0x08000000 + offset) + data
        return bytes([2, number, number ^ 255]) + body + bytes([sum(body) & 255])

    status = b"".join(bytes([0x21, op, 1]) for _ in range(2) for op in (5, 0x35, 0x15))
    full = read(0, 0x800000) if include_full_reads else b""
    parts = [full]
    for offset in range(START, END, SECTOR):
        payload = baseline[offset:offset + SECTOR]
        if offset == START:
            payload = b"\xff" * 8 + payload[8:]
        parts.extend([read(offset, SECTOR), status,
                      b"\x17" + offset.to_bytes(3, "little") + b"\x01\x00",
                      read(offset, SECTOR), status, b"\x07"])
        parts.extend(packet(i + 1, offset + 1024 * i, payload[1024 * i:1024 * (i + 1)]) for i in range(4))
        parts.extend([b"\x04", read(offset, SECTOR)])
    parts.extend([full, read(START, SECTOR), status, b"\x07",
                  packet(1, START, baseline[START:START + 1024]), b"\x04",
                  read(START, SECTOR), full])
    return b"".join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    private = (ROOT / "private").resolve()
    if not args.baseline.resolve().is_relative_to(private) or not args.output.resolve().is_relative_to(private):
        raise ValueError("Baseline and new output directory must stay private")
    data = expected(args.baseline.read_bytes())
    os.umask(0o077)
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    (args.output / "expected-serial-tx.bin").write_bytes(data)
    report = {"kind": "original_stock_bank1_writeback_test", "baseline_sha256": BASELINE_SHA,
              "target_range": {"start": START, "end_exclusive": END}, "sector_erases": 336,
              "staging_packets": 1344, "commit_packets": 1, "additional_commit_erases": 0,
              "full_reads": 3, "sector_reads": 1010, "status_requests": 4038,
              "tx_bytes": len(data), "tx_sha256": digest(data),
              "tool_sha256": digest(Path(__file__).read_bytes()),
              "limit": "No duplicate read frames modeled; extra ACKs require separate captured duplicate-frame evidence"}
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: report[key] for key in ("tx_bytes", "tx_sha256", "sector_erases")}))


if __name__ == "__main__":
    main()
