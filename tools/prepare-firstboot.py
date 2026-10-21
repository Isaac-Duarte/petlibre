#!/usr/bin/env python3
"""Freeze and review the UART-only standalone diagnostic offline; no device I/O.

Produces mapping and hypothetical stock-restoration evidence, not an executable
installation plan. The separate uart-standalone.py path handles the exact pinned
diagnostic; the existing ordinary stock-only CLI still refuses its valid marker.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


LAYOUT = module("firstboot_layout", "audit-ota-layout.py")
FACTORY = module("firstboot_factory", "audit-factory-mapping.py")
RECOVERY = module("firstboot_recovery", "plan-stock-recovery.py")
require = LAYOUT.require


def digest(data):
    return hashlib.sha256(data).hexdigest()


def mapping_cases(current, rom, image):
    payload = LAYOUT.container(image)
    regions = LAYOUT.regions(payload)
    split = regions[2]["payload_header_offset"]
    cases = []
    for bank, start in enumerate(FACTORY.BANKS):
        fixture = bytearray(current)
        fixture[start:start + len(payload)] = payload
        # This fixture deliberately selects just this bank. It is never sent.
        other = FACTORY.BANKS[1 - bank]
        fixture[other:other + 8] = b"\x00" * 8
        checked = FACTORY.execute(fixture, rom, bank)
        require(checked["stop"] == hex(FACTORY.EXIT) and checked["result"] == 1,
                "Actual factory instructions reject diagnostic headers")
        require(len(checked["configs"]) == 4 and checked["commands"] == [[0, 1], [1, 1]],
                "Unexpected factory mapper sequence")
        for index, physical, length in ((0, start, split), (1, start + split, len(payload) - split)):
            final = checked["configs"][index * 2 + 1]
            virtual = (0x0c000000, 0x0e000000)[index]
            require(final["start"] == virtual and final["end"] == virtual + length - 1 and
                    final["minus"] == 1 and final["offset"] == virtual - 0x08000000 - physical,
                    "Factory mapping does not cover packaged diagnostic")
        chosen = FACTORY.execute(fixture, rom)
        require(chosen["stop"] == "0x80004a0" and chosen["result"] == bank + 1,
                "Single-valid diagnostic bank selection differs")
        cases.append({"bank": bank, "bank_check": checked, "selection": chosen})
    return cases


def restoration_fixture(baseline, current, image):
    """Model exact bank-1 placement and later invalidation; never mutate hardware."""
    require(len(current) == len(baseline) == RECOVERY.CAPACITY, "Exact 8 MiB inputs required")
    require(current[:0x6000] == baseline[:0x6000] and
            current[RECOVERY.START:RECOVERY.END] == baseline[RECOVERY.START:RECOVERY.END],
            "Fresh snapshot must contain original factory and stock bank 1")
    payload = LAYOUT.container(image)
    require(len(payload) <= RECOVERY.END - RECOVERY.START, "Diagnostic exceeds stock bank footprint")
    installed = bytearray(current)
    installed[RECOVERY.START:RECOVERY.START + len(payload)] = payload
    try:
        RECOVERY.plan(baseline, installed)
    except ValueError as error:
        require("marker is still valid" in str(error), "Unexpected restoration refusal")
    else:
        raise ValueError("Existing stock-only guard unexpectedly accepts bootable custom bank")
    installed[RECOVERY.START:RECOVERY.START + 8] = b"\xff" * 8
    planned = RECOVERY.plan(baseline, installed)
    require(planned["proposed_snapshot_sha256"] == digest(current),
            "Hypothetical stock restoration loses current bytes")
    return {"live_custom_bank_refused_by_existing_cli": True,
            "invalidation_is_model_only": True,
            "hypothetical_invalidated_snapshot_sha256": digest(installed),
            "stock_restoration_plan": planned,
            "restores_exact_current_snapshot": True}


def prepare(build, current_path):
    manifest = json.loads((build / "manifest.json").read_text())
    require(manifest["profile"] == "plaf103-firstboot" and
            manifest.get("firstboot_startup_audit", {}).get("mains", {}).keys() == {"lp", "hp"},
            "Completed UART-only first-boot build required")
    for name, expected in manifest["inputs"].items():
        require(digest((ROOT / name).read_bytes()) == expected, "Build input changed: " + name)
    for name, expected in manifest["sdk_overrides"].items():
        require(digest((build / "sdk" / name).read_bytes()) == expected, "SDK overlay changed: " + name)
    for artifact in manifest["artifacts"]:
        data = (build / artifact["path"]).read_bytes()
        require(len(data) == artifact["bytes"] and digest(data) == artifact["sha256"],
                "Build artifact changed")
    candidate = next(a for a in manifest["artifacts"] if Path(a["path"]).name == "OTA_All.bin")
    image = (build / candidate["path"]).read_bytes()
    current = current_path.read_bytes()
    report_path = current_path.parent / "report.json"
    acquired = json.loads(report_path.read_text())
    require(acquired.get("mode") == "read-full" and acquired.get("status") == "complete" and
            acquired.get("result", {}).get("bytes") == RECOVERY.CAPACITY and
            acquired.get("result", {}).get("sha256") == digest(current), "Completed current read required")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(digest((current_path.parent / name).read_bytes()) == acquired.get(name + "_sha256"),
                "Current acquisition serial hash mismatch")
    baseline, rom = FACTORY.FLASH.read_bytes(), FACTORY.ROM.read_bytes()
    require(digest(baseline) == RECOVERY.BASELINE_SHA and digest(rom) == FACTORY.BOOT.ROM_SHA256,
            "Pinned baseline/ROM required")
    layout = LAYOUT.audit(build / candidate["path"], FACTORY.ORIGINAL,
                          ROOT / "firmware/plaf103-layout.json")
    report = {"created_utc": datetime.now(timezone.utc).isoformat(),
              "scope": "Offline standalone diagnostic preparation; no hardware or serial action",
              "candidate_sha256": digest(image), "candidate_bytes": len(image),
              "review_tool_sha256": {name: digest((ROOT / "tools" / name).read_bytes()) for name in
                                     ("prepare-firstboot.py", "audit-factory-mapping.py", "plan-stock-recovery.py")},
              "current_snapshot_sha256": digest(current),
              "build_manifest_sha256": digest((build / "manifest.json").read_bytes()),
              "current_read_report_sha256": digest(report_path.read_bytes()),
              "layout_audit": layout,
              "startup_audit": manifest["firstboot_startup_audit"],
              "factory_mapping_cases": mapping_cases(current, rom, image),
              "restoration_fixture": restoration_fixture(baseline, current, image),
              "physical_boot_verified": False, "ready_for_installation": False,
              "guarded_transport_tools": {name: digest((ROOT / "tools" / name).read_bytes()) for name in
                                           ("standalone-bank.py", "uart-standalone.py", "standalone-wire-policy.py")},
              "remaining": ["Establish a fresh supervised helper session and complete current read/status evidence",
                            "Review a new source-bound uart-standalone.py installation plan and verified backup copies",
                            "Perform supervised install, boot observation and original-stock restoration acceptance"],
              "limits": ["Factory mapper runs against modeled MMIO and sampled candidate ROM, not a full boot",
                         "SDK pre-main still configures clocks, stock-derived pulls and internal ADC",
                         "Flash high-speed startup can change status registers; no zero-side-effect claim",
                         "No physical GPIO, scheduler, watchdog or sustained runtime acceptance"]}
    return report, image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=Path, default=ROOT / "private/firmware-build/plaf103-firstboot")
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(all(p.resolve().is_relative_to((ROOT / "private").resolve())
                for p in (args.build, args.current, args.output)), "Private inputs/output required")
    report, image = prepare(args.build, args.current)
    os.umask(0o077)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    (args.output / "diagnostic-ota.bin").write_bytes(image)
    (args.output / "build-manifest.json").write_bytes((args.build / "manifest.json").read_bytes())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: report[key] for key in
                      ("candidate_sha256", "candidate_bytes", "physical_boot_verified", "ready_for_installation")}))


if __name__ == "__main__":
    main()
