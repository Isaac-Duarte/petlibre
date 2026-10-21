#!/usr/bin/env python3
"""Retain offline pinned-diagnostic install/stock-restore helper receipts.

Actual helper instructions execute against modeled UART/ROM/NOR. Full snapshots
read direct emulated backing. No serial device, network or hardware is accessed.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


BANK = module("emulated_standalone_bank", "standalone-bank.py")
SIM = module("emulated_standalone_helper", "emulate-recovery-session.py")
WIRE = module("emulated_standalone_wire", "standalone-wire-policy.py")
require, digest = BANK.require, BANK.digest


def phase(baseline, current, candidate, direction, output):
    planned = BANK.plan(baseline, current, candidate, direction)
    proposed, staged = BANK.images(baseline, current, candidate, direction)
    output.mkdir(mode=0o700)
    (output / "simulated-plan.json").write_text(json.dumps(planned, indent=2) + "\n")
    state = {"body_erases_with_marker_withheld": 0, "activation_calls": 0}

    class Peer(SIM.EmulatedHelperPort):
        def _step(self, uc, pc, size, data):
            if pc in (0x2146, 0x2048):
                offset = uc.reg_read(SIM.arm.UC_ARM_REG_R1 if pc == 0x2146 else SIM.arm.UC_ARM_REG_R0)
                if offset >= BANK.START + BANK.SIZE:
                    require(bytes(uc.mem_read(0x08000000 + BANK.START, 8)) == b"\xff" * 8,
                            "Body mutation with valid marker")
                    if pc == 0x2146:
                        state["body_erases_with_marker_withheld"] += 1
                if pc == 0x2048 and offset == BANK.START:
                    pointer = uc.reg_read(SIM.arm.UC_ARM_REG_R2)
                    if bytes(uc.mem_read(pointer, 8)) != b"\xff" * 8:
                        require(self.snapshot() == staged, "Activation before exact full staged image")
                        state["activation_calls"] += 1
            super()._step(uc, pc, size, data)

    offsets = [s["offset"] for s in planned["ordered_sector_payloads"]]
    peer = Peer(SIM.HELPER.read_bytes(), current, offsets, status_registers=(0, 2, 0))
    snapshots = []

    def snapshot():
        data = peer.snapshot()
        name = f"snapshot-{len(snapshots) + 1:02d}.bin"
        (output / name).write_bytes(data)
        snapshots.append({"file": name, "bytes": len(data), "sha256": digest(data)})
        return data

    receipt = BANK.run(peer, baseline, current, candidate, direction, planned, snapshot, output / "transaction")
    wire = bytes(peer.host_tx)
    require(wire == WIRE.expected(baseline, candidate, direction, include_full_reads=False),
            "Independent mutation wire profile differs")
    require(peer.snapshot() == proposed and len(snapshots) == 3, "Final full image/snapshot count differs")
    erases = [c["offset"] for c in peer.calls if c["name"] == "erase"]
    require(erases == offsets and state == {"body_erases_with_marker_withheld": 25, "activation_calls": 1},
            "Erase order or staged-marker observations differ")
    journal = BANK.C.inspect_journal(output / "transaction/journal.jsonl")
    require(journal["verified_terminal"] and journal["erase_order"] == offsets, "Receipt chain differs")
    (output / "emulated-host-tx.bin").write_bytes(wire)
    result = {"direction": direction, "status": "offline_verified", "physical_boot_verified": False,
              "current_sha256": digest(current), "proposed_sha256": digest(proposed),
              "snapshots": snapshots, "tx_bytes": len(wire), "tx_sha256": digest(wire),
              "independent_wire_policy_matched": True, "journal": journal, **state,
              "rewritten_sector_count": receipt["rewritten_sector_count"]}
    (output / "report.json").write_text(json.dumps(result, indent=2) + "\n")
    return proposed, result


def run(baseline, candidate, output):
    baseline = BANK.T.pinned_baseline(baseline)
    BANK.candidate_payload(candidate)
    require(output.resolve().is_relative_to((ROOT / "private").resolve()), "Private new output required")
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    current = bytearray(baseline)
    for offset in (0x3e8000, 0x650000):
        current[offset] ^= 1
    current = bytes(current)
    installed, install_report = phase(baseline, current, candidate, "install", output / "install")
    # Model a newer outside byte before restoration; no application is emulated.
    restore_source = bytearray(installed)
    restore_source[0x7cb001] ^= 1
    restored, restore_report = phase(baseline, bytes(restore_source), candidate, "restore", output / "restore")
    expected = bytearray(current)
    expected[0x7cb001] ^= 1
    require(restored == expected, "Round trip lost newer outside bytes")
    result = {"status": "offline_roundtrip_verified", "physical_boot_verified": False,
              "candidate_sha256": BANK.CANDIDATE_SHA, "baseline_sha256": digest(baseline),
              "helper_sha256": SIM.HELPER_SHA, "install": install_report, "restore": restore_report,
              "synthetic_newer_outside_bytes_preserved": True,
              "scope": "Actual helper instructions with modeled ROM/NOR; bulk snapshots use direct backing; no app execution",
              "tool_sha256": {n: digest((ROOT / "tools" / n).read_bytes()) for n in
                  ("emulate-standalone.py", "standalone-bank.py", "uart-standalone.py", "standalone-wire-policy.py",
                   "recovery-transaction.py", "recovery-sector-transport.py", "uart-flash-read.py", "flash-status.py",
                   "plan-stock-recovery.py", "emulate-recovery-session.py")}}
    (output / "report.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    require(all(p.resolve().is_relative_to((ROOT / "private").resolve())
                for p in (args.baseline, args.candidate, args.output)), "Private inputs/output required")
    os.umask(0o077)
    result = run(args.baseline.read_bytes(), args.candidate.read_bytes(), args.output)
    print(json.dumps({"status": result["status"], "sectors_per_direction": 26,
                      "physical_boot_verified": False}))


if __name__ == "__main__":
    main()
