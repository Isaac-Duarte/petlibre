#!/usr/bin/env python3
"""Retain actual-helper development round-trip evidence offline.

Bulk region acquisitions use direct modeled backing; sector transport, command
parsing and flash calls execute the pinned ARM helper. No physical port opens.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


CLI = module("emulated_development_cli", "flash/uart-development.py")
SIM = module("emulated_development_peer", "emulate/emulate-recovery-session.py")
WIRE = module("emulated_development_wire", "flash/development-wire-policy.py")
B = CLI.B


def phase(baseline, before, source, target, output):
    output.mkdir(mode=0o700)
    length = B.extent(source, target)
    current = before[B.START:B.START + length]
    planned = B.plan(source, target, current)
    proposed, staged = B.images(source, target, current)
    (output / "plan.json").write_bytes(B.canonical(planned) + b"\n")
    state = {"body_erases_with_marker_withheld": 0, "activation_calls": 0}

    class Peer(SIM.EmulatedHelperPort):
        def _step(self, uc, pc, size, data):
            if pc == 0x2146:
                offset = uc.reg_read(SIM.arm.UC_ARM_REG_R1)
                if offset != B.START:
                    B.require(bytes(uc.mem_read(0x08000000 + B.START, 8)) == b"\xff" * 8,
                              "Body erase before marker withholding")
                    state["body_erases_with_marker_withheld"] += 1
            if pc == 0x2048 and uc.reg_read(SIM.arm.UC_ARM_REG_R0) == B.START:
                pointer = uc.reg_read(SIM.arm.UC_ARM_REG_R2)
                if bytes(uc.mem_read(pointer, 8)) != b"\xff" * 8:
                    B.require(self.snapshot()[B.START:B.START + length] == staged,
                              "Activation before staged region verification")
                    state["activation_calls"] += 1
            super()._step(uc, pc, size, data)

    offsets = [s["offset"] for s in planned["ordered_sectors"]]
    peer = Peer(SIM.HELPER.read_bytes(), before, offsets or [B.START], status_registers=(0, 2, 0))
    observations = []

    def acquire():
        full = peer.snapshot()
        region = full[B.START:B.START + length]
        filename = f"application-{len(observations) + 1:02d}.bin"
        (output / filename).write_bytes(region)
        observations.append({"file": filename, "bytes": len(region), "sha256": B.digest(region)})
        return region, [full[a:a + n] for a, n in B.GUARDS]

    B.run(peer, baseline, source, target, current, planned, acquire, output / "transaction")
    after = peer.snapshot()
    expected_full = before[:B.START] + proposed + before[B.START + length:]
    B.require(after == expected_full, "Changed bytes outside intended region")
    actual = bytes(peer.host_tx)
    B.require(actual == WIRE.expected(source, target, current, include_acquisitions=False),
              "Independent mutation wire differs")
    physical = WIRE.expected(source, target, current)
    (output / "emulated-host-tx.bin").write_bytes(actual)
    (output / "expected-physical-tx.bin").write_bytes(physical)
    receipt = B.C.inspect_journal(output / "transaction/journal.jsonl")
    B.require(receipt["verified_terminal"] and receipt["erase_order"] == offsets,
              "Receipt or erase order differs")
    B.require(len(observations) == (3 if offsets else 1), "Unexpected acquisition count")
    result = {"status": "offline_verified", "physical_operation": False,
              "source_ota_sha256": B.digest(source), "target_ota_sha256": B.digest(target),
              "before_full_sha256": B.digest(before), "after_full_sha256": B.digest(after),
              "application_region_bytes": length, "changed_sectors": len(offsets),
              "region_acquisitions": observations,
              "bulk_read_bytes_in_execution": (length + sum(n for _, n in B.GUARDS)) * len(observations),
              "independent_mutation_wire_matches": True,
              "unscanned_outside_bytes_preserved_in_model": True,
              "expected_physical_tx_bytes": len(physical), "expected_physical_tx_sha256": B.digest(physical),
              "journal": receipt, **state,
              "scope": "Actual helper parser/sector instructions; modeled ROM/NOR and direct bulk backing reads; no app boot"}
    (output / "report.json").write_text(json.dumps(result, indent=2) + "\n")
    return after, result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "source", "target", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    baseline = B.T.pinned_baseline(CLI.private_file(args.baseline))
    source, _ = CLI.frozen(args.source)
    target, _ = CLI.frozen(args.target)
    stock = CLI.private_file(ROOT / "private/firmware/PLAF103_OTA_010115.bin")
    B.require(B.digest(stock) == B.L.STOCK_SHA256, "Original stock required")
    B.require(args.output.resolve().is_relative_to((ROOT / "private").resolve()), "New private output required")
    os.umask(0o077)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    current = bytearray(baseline)
    current[0x3e8000] ^= 1
    current[0x650000] ^= 1
    current = bytes(current)
    installed, first = phase(baseline, current, stock, source, args.output / "stock-to-source")
    updated, iteration = phase(baseline, installed, source, target, args.output / "changed-build")
    # Model newer data outside the application before returning to stock.
    updated = bytearray(updated)
    updated[0x7cb001] ^= 1
    restored, final = phase(baseline, bytes(updated), target, stock, args.output / "return-to-stock")
    expected = bytearray(current)
    expected[0x7cb001] ^= 1
    B.require(restored == expected, "Round trip lost outside data")
    result = {"status": "offline_development_roundtrip_verified", "physical_operation": False,
              "stock_to_source": first, "changed_build": iteration, "return_to_stock": final,
              "newer_outside_data_preserved": True}
    (args.output / "report.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "small_iteration_sectors": iteration["changed_sectors"],
                      "small_iteration_region_bytes": iteration["application_region_bytes"],
                      "physical_operation": False}))


if __name__ == "__main__":
    main()
