#!/usr/bin/env python3
"""Run a complete planned restore against in-memory helper/NOR, never hardware."""
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


TRANSACTION = module("recovery_transaction", "flash/recovery-transaction.py")
SIM = module("recovery_transaction_peer", "emulate/emulate-recovery-session.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"


def execute(output):
    baseline = BASELINE.read_bytes()
    TRANSACTION.require(TRANSACTION.digest(baseline) == TRANSACTION.PLAN.BASELINE_SHA, "Pinned baseline required")
    current = bytearray(baseline)
    current[TRANSACTION.PLAN.START] = 0
    current[TRANSACTION.PLAN.START + 4096 + 17] ^= 1
    current[0x3E8000] ^= 1
    current[0x600000] ^= 1
    current = bytes(current)
    manifest = TRANSACTION.PLAN.plan(baseline, current)
    offsets = [sector["offset"] for sector in manifest["ordered_sector_payloads"]]
    peer = SIM.EmulatedHelperPort(SIM.HELPER.read_bytes(), current, offsets, status_registers=(0, 2, 0x20))
    report = TRANSACTION.run_transaction(peer, baseline, current, manifest, peer.snapshot, output)
    inspected = TRANSACTION.inspect_journal(Path(output) / "journal.jsonl")
    TRANSACTION.require(inspected["verified_terminal"] and inspected["erase_order"] == offsets,
                        "Journal order or terminal verification failed")
    stored = json.loads((Path(output) / "report.json").read_text())
    TRANSACTION.require(stored["journal_head_sha256"] == inspected["head_sha256"], "Report/journal binding differs")
    files = ["flash/recovery-transaction.py", "flash/recovery-sector-transport.py", "emulate/emulate-recovery-session.py",
             "flash/plan-stock-recovery.py", "flash/uart-flash-read.py", "flash/flash-status.py", "emulate/emulate-recovery-transaction.py"]
    metadata = {"simulation_only": True, "serial_port_opened": False,
                "supplied_status_registers": [0, 2, 0x20],
                "baseline_sha256": TRANSACTION.digest(baseline), "helper_sha256": SIM.HELPER_SHA,
                "scenario": "Invalid old-stock marker plus damaged body sector; newer settings and unclassified bytes retained",
                "staged_sectors": len(offsets), "explicit_erase_calls": sum(c["name"] == "erase" for c in peer.calls),
                "eight_byte_program_calls": sum(c["name"] == "program" for c in peer.calls),
                "modeled_status_reads": sum(c["name"] == "read_status" for c in peer.calls),
                "final_snapshot_sha256": report["snapshot_sha256"], "journal": inspected,
                "tool_sha256": {name: TRANSACTION.digest((ROOT / "tools" / name).read_bytes()) for name in files},
                "limits": ["Helper parser/program instructions execute; ROM flash/UART and NOR behavior modeled",
                           "Full snapshots use direct emulator backing reads, not serial bulk acquisition",
                           "fsync ordering tested on host filesystem; real power-cut durability not established",
                           "Status values supplied; hardware busy/protection timing not reproduced",
                           "Physical identity/entry, protection/encryption and write acceptance unverified"]}
    with (Path(output) / "simulation.json").open("xb") as stream:
        stream.write(json.dumps(metadata, indent=2).encode() + b"\n")
        stream.flush()
        os.fsync(stream.fileno())
    TRANSACTION.sync_directory(Path(output))
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True,
                        help="New private directory under an existing parent")
    args = parser.parse_args()
    metadata = execute(args.output)
    print(f"PASS: {metadata['staged_sectors']} sectors staged, marker committed last, preserved current ranges verified in emulation")
    print("No physical device opened or flashed.")


if __name__ == "__main__":
    main()
