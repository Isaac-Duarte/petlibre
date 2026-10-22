#!/usr/bin/env python3
"""Check FW30 logic-analyzer captures of the motor driver for any drive.

Reads a `bench.py transients` session: raw sigrok bytes (one byte per sample,
bit n = channel Dn) plus its manifest naming the INA, INB, OUTA and OUTB
channels. The TC118AH drives the motor only when its inputs differ (forward
H/L, reverse L/H); L/L is standby and H/H is brake. Its outputs differ only
while driving.

Per cycle, a pass needs:
  - the capture to start with all four lines LOW (feeder unpowered), so the
    power-on itself was recorded;
  - no sample with INA != INB, and none with OUTA != OUTB;
  - boot mode: the inputs reach H/H (stopped startup's brake) and stay there
    to the end; loader mode: the inputs never leave L/L.
Every input or output disagreement is reported with its start and length,
however short; one sample is a sample period long.
"""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LINES = ("ina", "inb", "outa", "outb")
LIMIT = ('Digital levels at the analyzer threshold (about 1.4 V for fx2lafw clones), sampled at the recorded '
         'rate; a pulse shorter than one sample period can be missed. Floating driver outputs read LOW.')


def runs(data, mask):
    """(value, start, length) for each run of `byte & mask` in the capture."""
    out, start, current = [], 0, None
    for index, byte in enumerate(data):
        value = byte & mask
        if value != current:
            if current is not None:
                out.append((current, start, index - start))
            current, start = value, index
    if current is not None:
        out.append((current, start, len(data) - start))
    return out


def check_cycle(data, channels, mode, samplerate):
    bit = {name: 1 << channels[name] for name in LINES}
    mask = sum(bit.values())
    us = 1e6 / samplerate
    result = {"samples": len(data), "seconds": round(len(data) / samplerate, 6), "input_drive": [], "output_drive": []}
    if not data:
        return {**result, "outcome": "transients_empty"}
    timeline = runs(data, mask)
    for value, start, length in timeline:
        ina, inb = bool(value & bit["ina"]), bool(value & bit["inb"])
        outa, outb = bool(value & bit["outa"]), bool(value & bit["outb"])
        event = {"start_us": round(start * us, 3), "length_us": round(length * us, 3)}
        if ina != inb:
            result["input_drive"].append({**event, "direction": "forward" if ina else "reverse"})
        if outa != outb:
            result["output_drive"].append({**event, "high": "outa" if outa else "outb"})
    inputs = [(bool(v & bit["ina"]), bool(v & bit["inb"]), s, n) for v, s, n in timeline]
    brake_at = next((s for a, b, s, _ in inputs if a and b), None)
    result["brake_at_s"] = None if brake_at is None else round(brake_at / samplerate, 6)
    result["transitions"] = len(timeline) - 1
    if timeline[0][0] != 0:
        return {**result, "outcome": "transients_late_start"}
    if result["input_drive"]:
        return {**result, "outcome": "transients_input_drive"}
    if result["output_drive"]:
        return {**result, "outcome": "transients_output_drive"}
    if mode == "boot":
        a, b, _, _ = inputs[-1]
        if brake_at is None or not (a and b):
            return {**result, "outcome": "transients_no_brake"}
        # After the first brake the inputs must not return to standby.
        if any(not (x and y) for x, y, s, _ in inputs if s > brake_at):
            return {**result, "outcome": "transients_left_brake"}
    elif brake_at is not None:
        return {**result, "outcome": "transients_loader_driven"}
    return {**result, "outcome": "pass"}


def verify(capture):
    manifest = json.loads((capture / "manifest.json").read_text())
    if manifest.get("schema_version") != 1 or manifest.get("mode") not in ("boot", "loader"):
        raise ValueError("Unsupported transients manifest")
    channels = manifest["channels"]
    if set(channels) < set(LINES) or len({channels[n] for n in LINES}) != len(LINES) or \
            not all(0 <= channels[n] < 8 for n in LINES):
        raise ValueError("Manifest needs four distinct channels 0-7 for INA, INB, OUTA and OUTB")
    cycles = []
    for entry in manifest["cycles"]:
        data = (capture / entry["file"]).read_bytes()
        if len(data) != entry["bytes"]:
            raise ValueError(f"{entry['file']} changed since capture")
        cycles.append({"file": entry["file"], **check_cycle(data, channels, manifest["mode"], manifest["samplerate"])})
    passed = sum(c["outcome"] == "pass" for c in cycles)
    brakes = [c["brake_at_s"] for c in cycles if c.get("brake_at_s") is not None]
    return {"outcome": "pass" if cycles and passed == len(cycles) else "transients_fail",
            "mode": manifest["mode"], "samplerate": manifest["samplerate"], "cycles": len(cycles), "passed": passed,
            "input_drive_events": sum(len(c["input_drive"]) for c in cycles),
            "output_drive_events": sum(len(c["output_drive"]) for c in cycles),
            "brake_at_s": {"min": min(brakes), "max": max(brakes)} if brakes else None,
            "per_cycle": cycles, "limit": LIMIT}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not args.output.resolve().is_relative_to((ROOT / "private").resolve()):
        parser.error("Evidence output must stay under private/")
    report = verify(args.capture)
    with args.output.open("x") as stream:
        stream.write(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "per_cycle"}))
    return 0 if report["outcome"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
