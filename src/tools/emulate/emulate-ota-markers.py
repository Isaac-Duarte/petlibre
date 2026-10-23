#!/usr/bin/env python3
"""Run the planned OTA marker sequence through the actual factory loader.

Each modeled flash state of the FW10/FW11 design (docs/firmware/ota-design.md)
is handed to the factory loader's own bank-selection code (via
audit/audit-factory-mapping.py), which must pick the bank the design expects.
States cover every step, a marker cut short at each byte, and erased or
partly staged bodies. No step may reach the no-image wait loop.

The images are a replacement build in both roles: recovery in bank 0,
candidate in bank 1. Offline only; no device access.
"""
import argparse
from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("factory_mapping", ROOT / "src/tools/audit/audit-factory-mapping.py")
FM = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FM)
MARKER = b"81958711"
BANK_BYTES = 0x150000
WAIT = "0x8000482"


def place(flash, bank, image, marker=MARKER, body=True):
    """Write `image` (an OTA container) into `bank` with `marker` as its first bytes."""
    base = FM.BANKS[bank]
    flash[base:base + BANK_BYTES] = b"\xff" * BANK_BYTES
    payload = image[32:]
    if body:
        flash[base:base + len(payload)] = payload
    flash[base:base + len(MARKER)] = marker.ljust(len(MARKER), b"\xff")[:len(MARKER)]


def selected(flash, rom):
    result = FM.execute(flash, rom)
    if result["stop"] == WAIT:
        return "none"
    return {1: "bank0", 2: "bank1"}[result["result"]]


def states(image):
    """(label, builder, expected) for each modeled step of the update flow."""
    cleared = b"\x00" + MARKER[1:]
    out = [
        ("installed: normal in bank 1, recovery in bank 0", lambda f: (place(f, 0, image), place(f, 1, image)), "bank1"),
        ("give way: bank 1 marker first byte cleared", lambda f: (place(f, 0, image), place(f, 1, image, cleared)), "bank0"),
        ("download: bank 1 erased", lambda f: (place(f, 0, image), place(f, 1, image, b"", body=False)), "bank0"),
        ("download: bank 1 body staged, marker erased", lambda f: (place(f, 0, image), place(f, 1, image, b"")), "bank0"),
        ("activate: bank 1 marker written", lambda f: (place(f, 0, image), place(f, 1, image)), "bank1"),
        ("fallback: unconfirmed trial cleared", lambda f: (place(f, 0, image), place(f, 1, image, cleared)), "bank0"),
    ]
    # A cut during the 8-byte marker write leaves a prefix programmed and the rest erased.
    for n in range(1, len(MARKER)):
        out.append((f"cut: bank 1 marker {n}/8 bytes", lambda f, n=n: (place(f, 0, image), place(f, 1, image, MARKER[:n])),
                    "bank0"))
    # Without a recovery image, a cleared bank 1 leaves nothing to boot: the design forbids giving way then.
    out.append(("forbidden: give way with no recovery", lambda f: (place(f, 0, image, b"\x00" + MARKER[1:]),
                                                                   place(f, 1, image, cleared)), "none"))
    return out


def run(flash, rom, image):
    results = []
    for label, build, expected in states(image):
        f = bytearray(flash)
        build(f)
        got = selected(f, rom)
        results.append({"state": label, "expected": expected, "selected": got, "ok": got == expected})
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--image", type=Path, required=True, help="A reviewed replacement OTA container")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if not args.report.resolve().is_relative_to((ROOT / "private").resolve()):
        parser.error("Evidence output must stay under private/")
    flash, rom = bytearray(FM.FLASH.read_bytes()), FM.ROM.read_bytes()
    FM.BOOT.require(FM.BOOT.digest(flash) == FM.BOOT.FLASH_SHA256, "Unexpected full backup hash")
    FM.BOOT.require(FM.BOOT.digest(rom) == FM.BOOT.ROM_SHA256, "Unexpected ROM hash")
    image = args.image.read_bytes()
    results = run(flash, rom, image)
    report = {"created_utc": datetime.now(timezone.utc).isoformat(), "image_sha256": FM.BOOT.digest(image),
              "states": results, "outcome": "pass" if all(r["ok"] for r in results) else "fail",
              "limits": ["Factory selection slice from supplied flash, not a full reset or a power cut inside one byte",
                         "A marker cut is modeled as a programmed prefix with the rest erased"]}
    with args.report.open("x") as stream:
        stream.write(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"outcome": report["outcome"], "states": len(results)}))
    return 0 if report["outcome"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
