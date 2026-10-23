#!/usr/bin/env python3
"""Plan sector-scoped stock recovery offline. No serial, erase or program action.

Uses the verified research-unit baseline and a separately acquired current full
snapshot. Emits private sector payloads and preconditions, never an executable
flash command. Preserve all current bytes outside the selected application bank.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASELINE_SHA = "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc"
CAPACITY = 0x800000
SECTOR = 0x1000
START, END = 0x186000, 0x2D6000


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(value, message):
    if not value:
        raise ValueError(message)


def plan(baseline, current):
    require(digest(baseline) == BASELINE_SHA, "Not the verified research-unit baseline")
    require(len(baseline) == len(current) == CAPACITY, "Need two exact 8 MiB snapshots")
    require(current[:0x6000] == baseline[:0x6000], "Factory/pre-application bytes differ; stop for investigation")
    sectors = []
    proposed = bytearray(current)
    for offset in range(START, END, SECTOR):
        before, after = current[offset:offset + SECTOR], baseline[offset:offset + SECTOR]
        if before != after:
            staged = b"\xff" * 8 + after[8:] if offset == START else after
            sectors.append({"offset": offset, "bytes": SECTOR,
                            "before_sha256": digest(before), "after_sha256": digest(after),
                            "stage_sha256": digest(staged), "stage_required": before != staged,
                            "marker_withheld": offset == START,
                            "payload": f"sector-{offset:06x}.bin"})
            proposed[offset:offset + SECTOR] = after
    require(proposed[:START] == current[:START] and proposed[END:] == current[END:],
            "Proposed bytes escape application bank 1")
    require(proposed[START:END] == baseline[START:END], "Proposed bank is not exact baseline stock")
    require(not sectors or current[START:START + 8] != baseline[START:START + 8],
            "Target marker is still valid; refuse an in-place rewrite of a bootable bank")
    # Stage the marker sector last with eight erased bytes. Activation is a
    # separate no-erase pass after full-image and preserve-range verification.
    sectors.sort(key=lambda entry: (entry["offset"] == START, entry["offset"]))
    preserved = [{"start": a, "end_exclusive": b, "sha256": digest(current[a:b])}
                 for a, b in ((0, START), (END, CAPACITY))]
    activation = None
    if sectors:
        staged_image = bytearray(proposed)
        staged_image[START:START + 8] = b"\xff" * 8
        activation = {"marker_offset": START, "marker_bytes": 8, "additional_erase": False,
                      "staged_snapshot_sha256": digest(staged_image),
                      "payload": "marker-commit-1k.bin", "payload_bytes": 1024,
                      "payload_sha256": digest(baseline[START:START + 1024])}
    return {"schema_version": 2, "created_utc": datetime.now(timezone.utc).isoformat(),
            "baseline_sha256": digest(baseline), "current_snapshot_sha256": digest(current),
            "proposed_snapshot_sha256": digest(proposed), "target_bank": 1,
            "target_range": {"start": START, "end_exclusive": END}, "sector_bytes": SECTOR,
            "changed_sector_count": len(sectors), "ordered_sector_payloads": sectors,
            "activation": activation,
            "preserve_current_ranges": preserved,
            "scope": "Offline plan only; no physical operation authorized or executed by this tool",
            "required_before_any_write": [
                "Operator must establish current snapshot is from the research feeder; hashes do not identify hardware",
                "Prove independent hardware loader entry and obtain stable current readback before stock can run",
                "Review pinned helper write/erase protocol and accepted protection/encryption behavior",
                "Use only sector erase; never chip erase or blanket protection/status-register clearing",
                "Check each sector against before_sha256 immediately before erasing; stop on mismatch",
                "Read back staged sectors against stage_sha256, withholding the first eight marker bytes",
                "Verify staged full image and current preserved ranges before a separate no-erase marker commit",
                "Verify full target bank and all preserved current ranges before attempting stock boot",
                "No blind retries, automatic fallback writes or writes to factory/OTP/eFuse/settings"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--current", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    baseline, current = args.baseline.read_bytes(), args.current.read_bytes()
    report = plan(baseline, current)
    require(args.output.resolve().is_relative_to((ROOT / "private").resolve()), "Output must be private")
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    for sector in report["ordered_sector_payloads"]:
        offset = sector["offset"]
        payload = baseline[offset:offset + SECTOR]
        if sector["marker_withheld"]:
            payload = b"\xff" * 8 + payload[8:]
        require(digest(payload) == sector["stage_sha256"], "Staging payload hash differs")
        with (args.output / sector["payload"]).open("xb") as handle:
            handle.write(payload)
    if report["activation"]:
        with (args.output / report["activation"]["payload"]).open("xb") as handle:
            handle.write(baseline[START:START + 1024])
    with (args.output / "plan.json").open("x") as handle:
        handle.write(json.dumps(report, indent=2) + "\n")
    print(f"Offline plan: {report['changed_sector_count']} application-bank sectors; no device I/O")


if __name__ == "__main__":
    main()
