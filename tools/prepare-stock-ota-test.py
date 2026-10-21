#!/usr/bin/env python3
"""Prepare one fixed stock-version experiment offline; never contacts a feeder."""

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
LAYOUT = ROOT / "firmware/plaf103-layout.json"
VERSION_OFFSET = 0xEDC95
ORIGINAL_VERSION = b"1.1.15\0"
TEST_VERSION = b"1.1.16\0"
SPEC = importlib.util.spec_from_file_location("ota_layout", ROOT / "tools/audit-ota-layout.py")
OTA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OTA)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify_patch(original, candidate):
    """Reject any change beyond the fixed version byte and container checksum."""
    OTA.require(digest(original) == OTA.STOCK_SHA256, "Not the verified stock image")
    OTA.require(len(candidate) == len(original), "Candidate size changed")
    OTA.require(original.count(ORIGINAL_VERSION) == 1 and
                original[VERSION_OFFSET:VERSION_OFFSET + 7] == ORIGINAL_VERSION,
                "Stock version literal does not match the reviewed location")
    OTA.require(candidate[VERSION_OFFSET:VERSION_OFFSET + 7] == TEST_VERSION,
                "Unexpected test version")
    original_payload = OTA.container(original)
    candidate_payload = OTA.container(candidate)
    OTA.require(OTA.regions(original_payload) == OTA.regions(candidate_payload),
                "Image layout changed")
    changes = [index for index, (before, after) in enumerate(zip(original, candidate))
               if before != after]
    OTA.require(VERSION_OFFSET + 5 in changes and
                set(changes) <= {16, 17, 18, 19, VERSION_OFFSET + 5},
                "Unexpected changed byte outside version/checksum")
    return [{"file_offset": f"0x{index:08x}", "before": original[index],
             "after": candidate[index]} for index in changes]


def prepare(stock, output):
    # Use the same reviewed layout gates as a model SDK build. They do not
    # establish boot acceptance or provide a physical recovery guarantee.
    reference_audit = OTA.audit(stock, stock, LAYOUT)
    original = stock.read_bytes()
    candidate = bytearray(original)
    candidate[VERSION_OFFSET:VERSION_OFFSET + 7] = TEST_VERSION
    struct.pack_into("<I", candidate, 16, sum(candidate[32:]) & 0xFFFFFFFF)
    candidate = bytes(candidate)
    changes = verify_patch(original, candidate)
    # Never replace an existing experiment or write images outside ignored data.
    output = output.resolve()
    OTA.require(output.is_relative_to((ROOT / "private").resolve()),
                "Output must be inside the ignored private directory")
    output.mkdir(parents=True, exist_ok=False)
    original_path = output / "original-1.1.15.bin"
    candidate_path = output / "version-test-1.1.16.bin"
    original_path.write_bytes(original)
    candidate_path.write_bytes(candidate)
    report = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "kind": "modified-stock-version-test",
        "original_version": "1.1.15",
        "test_version": "1.1.16",
        "original_sha256": digest(original),
        "candidate_sha256": digest(candidate),
        "bytes": len(original),
        "changed_bytes": changes,
        "reference_audit": reference_audit,
        "candidate_audit": OTA.audit(candidate_path, stock, LAYOUT),
        "executable_instructions_changed": False,
        "full_flash_backup": False,
        "boot_acceptance_verified": False,
        "wifi_restore_verified": False,
        "device_command_sent": False,
    }
    (output / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stock", type=Path, default=STOCK)
    parser.add_argument("--output", type=Path, required=True,
                        help="New experiment directory under private/")
    args = parser.parse_args()
    report = prepare(args.stock, args.output)
    print(json.dumps({key: report[key] for key in
                      ("kind", "bytes", "candidate_sha256", "changed_bytes",
                       "boot_acceptance_verified", "wifi_restore_verified",
                       "device_command_sent")}, indent=2))


if __name__ == "__main__":
    main()
