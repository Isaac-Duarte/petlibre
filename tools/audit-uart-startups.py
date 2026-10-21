#!/usr/bin/env python3
"""Audit saved UART startup/reset evidence offline; emit no raw device text.

No serial, network, device, or flash operation is performed. Reset-reason
fixtures execute pinned factory/ROM instructions against ordinary modeled RAM.
"""
import argparse
from bisect import bisect_right
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import struct

ROOT = Path(__file__).resolve().parents[1]
FLASH_SHA = "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc"
ROM_SHA = "073a80be644d776a007cd22fc4c10490ce75655de450c6f65849b254f2140fa9"
FLASH = ROOT / "private/fixtures/baseline.bin"
ROM = ROOT / "private/firmware/plaf103-sdk-rom-inputs/km4-rom-text-symbols.bin"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def timestamp(value):
    result = datetime.fromisoformat(value)
    require(result.tzinfo is not None, "Timestamp requires a timezone")
    return result


def analyze(rx, tx, events, metadata):
    """Validate finalized evidence, then extract only fixed markers/numbers."""
    require(metadata.get("status") == "complete", "Capture must be complete")
    for direction, data in (("rx", rx), ("tx", tx)):
        hashes = [metadata[key] for key in
                  (f"{direction}_sha256", f"serial-{direction}.bin_sha256")
                  if key in metadata]
        require(hashes, "Finalized capture hash missing")
        require(all(value == digest(data) for value in hashes), "Capture hash mismatch")
    offsets = {"rx": 0, "tx": 0}
    rx_events, previous = [], None
    for event in events:
        direction = event.get("direction")
        require(direction in offsets, "Invalid event direction")
        size, offset = event.get("bytes"), event.get("offset")
        require(type(size) is int and size > 0 and type(offset) is int,
                "Invalid event extent")
        require(offset == offsets[direction], "Noncontiguous event offsets")
        when = timestamp(event["utc"])
        require(previous is None or when >= previous, "Event timestamps moved backward")
        previous = when
        offsets[direction] += size
        if direction == "rx":
            rx_events.append((offset, when))
    require(offsets == {"rx": len(rx), "tx": len(tx)}, "Event coverage mismatch")
    starts = [offset for offset, _ in rx_events]

    def observation(match, **fields):
        when = rx_events[bisect_right(starts, match.start()) - 1][1]
        return {"offset": match.start(), "utc": when.isoformat(), **fields}

    apps = [observation(m, version=m[1].decode("ascii")) for m in
            re.finditer(rb"app_main:version:(\d+\.\d+\.\d+)(?![\d.])", rx)]
    flashes = [observation(m) for m in re.finditer(rb"Flash ID:", rx)]
    associations = [observation(m, status=int(m[1])) for m in
                    re.finditer(rb"Recv AssocRsp with status_code=(\d+)\b", rx)]
    reasons = [observation(m, value=f"{int(m[1], 16):08x}") for m in re.finditer(
        rb"\[ota_persist\] recover enter reg3=[0-9a-fA-F]{8} "
        rb"reg4=[0-9a-fA-F]{8} boot=([0-9a-fA-F]{8})\b", rx)]
    intervals = []
    for left, right in zip(apps, apps[1:]):
        interval = {"previous_start": left, "next_start": right,
                    "seconds": (timestamp(right["utc"]) - timestamp(left["utc"])).total_seconds()}
        rejected = [x for x in associations if left["offset"] < x["offset"] < right["offset"]
                    and x["status"] != 0]
        if rejected:
            last = rejected[-1]
            next_flash = next((x for x in flashes if last["offset"] < x["offset"]
                               < right["offset"]), None)
            interval["last_association_rejection"] = last
            if next_flash:
                interval["rejection_to_flash_seconds"] = (
                    timestamp(next_flash["utc"]) - timestamp(last["utc"])).total_seconds()
        intervals.append(interval)
    return {"rx_bytes": len(rx), "rx_sha256": digest(rx), "tx_bytes": len(tx),
            "tx_sha256": digest(tx), "events": len(events), "application_starts": apps,
            "flash_initializations": flashes, "association_responses": associations,
            "reported_boot_reasons": reasons, "startup_intervals": intervals,
            "recognized_fault_marker_seen": any(marker in rx for marker in
                (b"Hard Fault", b"HardFault", b"Fault on thread")),
            "limits": ["UART timestamps identify receive chunks, not exact hardware timing",
                       "Boot reasons are observed only when the stock logger reports them",
                       "No reset PC, watchdog refresh trace, or physical reset cause is inferred"]}


def reset_cases(flash, rom):
    """Execute actual factory reason preservation and candidate ROM getter."""
    require(digest(flash) == FLASH_SHA, "Wrong factory backup")
    require(digest(rom) == ROM_SHA, "Wrong candidate ROM text")
    from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
    from unicorn import arm_const as arm

    results = []
    cases = (("cold", 0, 0), ("km4_watchdog", 16, 0x100000),
             ("km4_system", 8, 0x80000), ("km4_system_and_watchdog", 24, 0x80000),
             ("km0_watchdog", 2, 0x20000))
    for name, backup, expected in cases:
        uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
        uc.ctl_set_cpu_model(arm.UC_CPU_ARM_CORTEX_M4)
        uc.mem_map(0x08000000, 0x1000)
        uc.mem_write(0x08000000, flash[:0x1000])
        uc.mem_map(0x1010A000, (len(rom) + 4095) & ~4095)
        uc.mem_write(0x1010A000, rom)
        uc.mem_map(0x80000, 0x10000)
        uc.mem_map(0x48000000, 0x1000)
        uc.mem_write(0x480003C0, struct.pack("<I", backup))
        stops = []

        def step(machine, pc, _size, _data):
            if pc == 0x8F000:
                stops.append(pc)
                machine.emu_stop()
            else:
                require(0x080001CC <= pc < 0x08000224 or 0x1010A928 <= pc < 0x1010A932,
                        "Unexpected reset-decoder instruction")

        uc.hook_add(UC_HOOK_CODE, step)
        for entry in (0x080001CD, 0x1010A929):
            uc.reg_write(arm.UC_ARM_REG_SP, 0x8E000)
            uc.reg_write(arm.UC_ARM_REG_LR, 0x8F001)
            uc.emu_start(entry, 0x8F002, count=200, timeout=1_000_000)
        require(len(stops) == 2, "Reset decoder did not return")
        actual = uc.reg_read(arm.UC_ARM_REG_R0)
        preserved = struct.unpack("<I", uc.mem_read(0x480003F8, 4))[0]
        require(actual == preserved == expected, "Unexpected reset decoding")
        results.append({"case": name, "supplied_backup0": hex(backup),
                        "preserved_reason": f"{preserved:08x}", "rom_reason": f"{actual:08x}"})
    return {"flash_sha256": FLASH_SHA, "candidate_rom_text_sha256": ROM_SHA,
            "factory_function": "0x080001cc", "rom_function": "0x1010a928",
            "cases": results, "limit": "Supplied MMIO state; no full reset or hardware execution"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--verify-reset-decoder", action="store_true")
    args = parser.parse_args()
    private = (ROOT / "private").resolve()
    require(args.capture.resolve().is_relative_to(private), "Capture must stay private")
    require(args.output.resolve().is_relative_to(private) and not args.output.exists(),
            "Output must be a new private file")
    os.umask(0o077)
    capture = args.capture
    raw_events = (capture / "serial-events.jsonl").read_bytes()
    report = analyze((capture / "serial-rx.bin").read_bytes(),
                     (capture / "serial-tx.bin").read_bytes(),
                     [json.loads(line) for line in raw_events.splitlines()],
                     json.loads((capture / "report.json").read_text()))
    report.update(created_utc=datetime.now(timezone.utc).isoformat(),
                  tool_sha256=digest(Path(__file__).read_bytes()),
                  events_sha256=digest(raw_events),
                  capture_report_sha256=digest((capture / "report.json").read_bytes()))
    if args.verify_reset_decoder:
        report["reset_decoder"] = reset_cases(FLASH.read_bytes(), ROM.read_bytes())
    args.output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with args.output.open("x") as stream:
        stream.write(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"application_starts": len(report["application_starts"]),
                      "flash_initializations": len(report["flash_initializations"]),
                      "association_statuses": [x["status"] for x in report["association_responses"]],
                      "boot_reasons": [x["value"] for x in report["reported_boot_reasons"]],
                      "transmitted_bytes": report["tx_bytes"]}))


if __name__ == "__main__":
    main()
