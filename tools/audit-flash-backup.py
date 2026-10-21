#!/usr/bin/env python3
"""Compare two existing full-flash reads offline. No serial, network or flash writes."""

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import struct

ROOT = Path(__file__).resolve().parents[1]
LAYOUT = ROOT / "firmware/plaf103-layout.json"
SPEC = importlib.util.spec_from_file_location("ota_layout", ROOT / "tools/audit-ota-layout.py")
OTA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OTA)
KNOWN_IMAGES = {
    OTA.STOCK_SHA256: "original-1.1.15",
    "3cd8d0454f3eee84b6fc81fdf30999b1527e7ed2aad9e8ec3fadbd906da86503": "version-marker-1.1.16",
    "fb5a04f3bdfe5b42555b684db176764018e2200ff6a4a784359d356ada8a1804": "c-probe-1.1.17",
    "9af0ce801c538f7ff4b6edbe84f4d250791c5dedefbf01f0b18958b7123df6e7": "rust-probe-1.1.17",
}
MAX_BYTES = 64 * 1024 * 1024
MAX_RUNS = 128


def digest(data):
    return hashlib.sha256(data).hexdigest()


def number(text):
    return int(text, 0)


def snapshot(path):
    """Read a stable regular-file snapshot, retaining no plaintext in reports."""
    path = path.resolve(strict=True)
    info = path.stat()
    OTA.require(stat.S_ISREG(info.st_mode), "Input must be a regular file")
    with path.open("rb") as handle:
        before = os.fstat(handle.fileno())
        OTA.require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= MAX_BYTES,
                    "Input must be a nonempty regular file no larger than 64 MiB")
        data = handle.read(MAX_BYTES + 1)
        after = os.fstat(handle.fileno())
    identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    OTA.require(identity(before) == identity(after) == identity(path.stat())
                and len(data) == before.st_size, "Input changed during the audit")
    return data, {"path": str(path), "bytes": len(data), "sha256": digest(data)}, (before.st_dev, before.st_ino)


def differences(first, second):
    if first == second:
        return {"different_bytes": 0, "runs": [], "total_runs": 0, "runs_truncated": False}
    count = 0
    runs = []
    total_runs = 0
    start = None
    # A missing byte is a difference as well; partial reads cannot pass.
    length = max(len(first), len(second))
    for offset in range(length):
        different = offset >= min(len(first), len(second)) or first[offset] != second[offset]
        if different:
            count += 1
            if start is None:
                start = offset
        elif start is not None:
            if len(runs) < MAX_RUNS:
                runs.append({"start": start, "end_exclusive": offset})
            total_runs += 1
            start = None
    if start is not None:
        if len(runs) < MAX_RUNS:
            runs.append({"start": start, "end_exclusive": length})
        total_runs += 1
    return {"different_bytes": count, "runs": runs, "total_runs": total_runs,
            "runs_truncated": total_runs > len(runs)}


def suspicious_patterns(data):
    issues = []
    if data.count(b"\xff") == len(data):
        issues.append("entire_read_is_erased_0xff")
    if data.count(b"\x00") == len(data):
        issues.append("entire_read_is_zero")
    # Repeated halves can indicate an address alias or an incorrectly doubled
    # read. They are not proof of either; flag them for investigation.
    if len(data) % 2 == 0 and data[:len(data) // 2] == data[len(data) // 2:]:
        issues.append("identical_halves_require_alias_or_content_investigation")
    return issues


def reference(path):
    if path is None:
        return None
    data, _, _ = snapshot(path)
    OTA.require(digest(data) == OTA.STOCK_SHA256, "Reference is not the pinned original 1.1.15 OTA")
    payload = OTA.container(data)
    OTA.regions(payload)
    return data


def banks(data, layout, original):
    result = []
    size = layout["candidate_payload_cap_bytes"]
    for index, value in enumerate(layout["ota_bank_offsets"]):
        start = int(value, 16)
        end = start + size
        item = {"index": index, "start": start, "end_exclusive": end,
                "stock_footprint_covered": end <= len(data), "currently_active": "unknown"}
        if end <= len(data):
            payload = data[start:end]
            marker_matches = payload[:8] == struct.pack("<2I", *OTA.IMAGE_MAGIC)
            item.update({"payload_sha256": digest(payload), "marker_matches_known_magic": marker_matches})
            if original is not None:
                # The updater invalidates the previous bank's first marker.
                # Restore ONLY eight bytes in RAM to identify its saved body;
                # never emit or recommend writing this normalized image.
                normalized = original[32:40] + payload[8:]
                container = bytearray(original[:32] + normalized)
                struct.pack_into("<I", container, 16, sum(normalized) & 0xFFFFFFFF)
                item["image_after_marker_normalization"] = KNOWN_IMAGES.get(digest(container), "unrecognized")
                item["marker_normalization_bytes"] = 8
                item["matches_original_except_marker"] = normalized == original[32:]
        result.append(item)
    return result


def audit(first_path, second_path, capacity, unit_label, stock_path=None, capacity_evidence=None):
    OTA.require(isinstance(capacity, int) and 0 < capacity <= MAX_BYTES and capacity % 4096 == 0,
                "Capacity must be explicit, positive, sector aligned and no larger than 64 MiB")
    OTA.require(re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", unit_label) is not None,
                "Use a short non-secret unit label, not credentials or a MAC address")
    layout = json.loads(LAYOUT.read_text())
    OTA.require(layout["schema_version"] == 1 and layout["sector_bytes"] == 4096
                and layout["stock_sha256"] == OTA.STOCK_SHA256, "Unsupported layout")
    first, first_info, first_identity = snapshot(first_path)
    second, second_info, second_identity = snapshot(second_path)
    OTA.require(first_identity != second_identity, "Two paths to the same file are not separate reads")
    original = reference(stock_path)
    evidence = None
    if capacity_evidence is not None:
        _, evidence, _ = snapshot(capacity_evidence)
    diff = differences(first, second)
    lengths_match = len(first) == len(second) == capacity
    anomalies = {"first": suspicious_patterns(first), "second": suspicious_patterns(second)}
    preserve = []
    for area in layout["known_preserve_ranges"]:
        start, end = int(area["start"], 16), int(area["end_exclusive"], 16)
        covered = end <= min(len(first), len(second))
        entry = {"name": area["name"], "start": start, "end_exclusive": end, "covered_in_both": covered}
        if covered:
            a, b = first[start:end], second[start:end]
            entry.update({"first_sha256": digest(a), "second_sha256": digest(b),
                          "identical": a == b, "different_bytes": sum(x != y for x, y in zip(a, b))})
        preserve.append(entry)
    changed_sectors = []
    for start in range(0, max(len(first), len(second)), 4096):
        a, b = first[start:start + 4096], second[start:start + 4096]
        if a != b:
            changed_sectors.append({"start": start,
                "known_preserve_areas": [r["name"] for r in preserve if start < r["end_exclusive"] and r["start"] < start + 4096]})
    all_areas_covered = all(r["covered_in_both"] for r in preserve)
    passed = lengths_match and diff["different_bytes"] == 0 and not any(anomalies.values()) and all_areas_covered
    return {
        "schema_version": 1, "created_at": datetime.now(timezone.utc).isoformat(),
        "unit_label": unit_label, "operator_supplied_capacity_bytes": capacity,
        "capacity_evidence": evidence, "capacity_evidence_interpreted": False,
        "inputs": [first_info, second_info],
        "checks": {"lengths_match_supplied_capacity": lengths_match,
                   "byte_identical": diff["different_bytes"] == 0, "no_detected_pattern_anomalies": not any(anomalies.values()),
                   "all_observed_preserve_areas_covered": all_areas_covered},
        "comparison_passed": passed, "differences": diff, "changed_sectors": changed_sectors,
        "pattern_anomalies": anomalies, "known_preserve_areas": preserve,
        "application_banks": {"first": banks(first, layout, original), "second": banks(second, layout, original)},
        "stock_reference_sha256": digest(original) if original is not None else None,
        "limits": {"same_physical_unit_verified": False, "independent_acquisitions_verified": False,
                   "physical_capacity_verified": False, "read_method_verified_non_erasing": False,
                   "complete_partition_map": False, "failed_boot_recovery_verified": False,
                   "boot_selection_verified": False, "restorable_backup_verified": False},
        "scope": "Offline comparison of supplied raw reads, not authorization or a procedure to flash or clone a unit",
        "tool_sha256": digest(Path(__file__).read_bytes()), "layout_sha256": digest(LAYOUT.read_bytes()),
    }


def write_report(path, report, input_paths):
    OTA.require(not os.path.lexists(path), "Report already exists; choose a new path")
    path = path.resolve()
    OTA.require(path.is_relative_to((ROOT / "private").resolve()), "Report must be under ignored private/")
    OTA.require(path not in {p.resolve() for p in input_paths}, "Report cannot overwrite an input")
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    # No overwrite, including existing symlinks; reports can contain private paths.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as handle:
        json.dump(report, handle, indent=2)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    parser.add_argument("--capacity-bytes", type=number, required=True,
                        help="Independently established chip capacity; decimal or 0x-prefixed, never guessed from these files")
    parser.add_argument("--unit-label", required=True)
    parser.add_argument("--stock-reference", type=Path,
                        help="Optional pinned original OTA for bank-body identification; not itself a full backup")
    parser.add_argument("--capacity-evidence", type=Path,
                        help="Optional existing evidence file; recorded by hash, never interpreted as proof")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    try:
        report = audit(args.first, args.second, args.capacity_bytes, args.unit_label,
                       args.stock_reference, args.capacity_evidence)
        path = write_report(args.report, report, [args.first, args.second] +
                            [p for p in [args.stock_reference, args.capacity_evidence] if p is not None])
        print(f"{'PASS' if report['comparison_passed'] else 'REVIEW REQUIRED'}: offline read comparison; report: {path}")
        print("This does not verify physical capacity, acquisition independence or recovery.")
        return 0 if report["comparison_passed"] else 1
    except (OSError, ValueError, KeyError, TypeError, struct.error) as error:
        parser.exit(2, f"error: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
