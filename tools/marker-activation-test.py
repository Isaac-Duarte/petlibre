#!/usr/bin/env python3
"""Injected-transport original-stock write-back tests; serial lifecycle is external.

Only an exact original bank is eligible. One explicit marker-sector erase and
four staged packets precede full-image verification and a separate no-erase
one-packet marker commit. Explicit full-bank mode also rewrites every body
sector after withholding the marker. Failed exchanges are never replayed.
"""
import importlib.util
from pathlib import Path
import uuid

ROOT = Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


COORDINATOR = module("marker_test_coordinator", "recovery-transaction.py")
TRANSPORT = COORDINATOR.SECTORS
READER = TRANSPORT.READER
digest, require = TRANSPORT.digest, TRANSPORT.require
START, END, SIZE = TRANSPORT.START, TRANSPORT.END, TRANSPORT.SECTOR
MARKER_KIND = "original_stock_marker_activation_test"
BANK_KIND = "original_stock_bank1_writeback_test"


def plan(baseline, current, *, full_bank=False):
    require(type(full_bank) is bool, "Explicit boolean full-bank selection required")
    baseline = TRANSPORT.pinned_baseline(baseline)
    require(isinstance(current, bytes) and len(current) == len(baseline), "Exact current 8 MiB snapshot required")
    require(current[:0x6000] == baseline[:0x6000], "Factory bytes differ")
    require(current[START:END] == baseline[START:END], "Write-back test requires an exact original stock bank")
    staged = current[:START] + b"\xff" * 8 + current[START + 8:]
    offsets = list(range(START, END if full_bank else START + SIZE, SIZE))
    target_end = END if full_bank else START + SIZE
    result = {"schema_version": 1, "kind": BANK_KIND if full_bank else MARKER_KIND,
            "baseline_sha256": digest(baseline), "current_snapshot_sha256": digest(current),
            "target_range": {"start": START, "end_exclusive": target_end},
            "staged_snapshot_sha256": digest(staged), "proposed_snapshot_sha256": digest(current),
            "stage_sector_sha256": digest(TRANSPORT.sector_payload(baseline, START)),
            "marker_commit_sha256": digest(baseline[START:START + 1024]),
            "explicit_sector_erases": len(offsets), "staging_packets": len(offsets) * 4, "commit_packets": 1,
            "additional_commit_erases": 0,
            "preserve_current_ranges": [{"start": 0, "end_exclusive": START, "sha256": digest(current[:START])},
                                        {"start": target_end, "end_exclusive": len(current),
                                         "sha256": digest(current[target_end:])}],
            "scope": "Intentional temporary marker removal; no application boot until verified original activation"}
    if full_bank:
        result["ordered_sector_payloads"] = [
            {"offset": offset, "bytes": SIZE,
             "before_sha256": digest(current[offset:offset + SIZE]),
             "stage_sha256": digest(TRANSPORT.sector_payload(baseline, offset)),
             "marker_withheld": offset == START}
            for offset in offsets]
        result["scope"] = ("Explicit original-stock full-bank write-back acceptance; marker sector first, "
                           "all body sectors next, separate no-erase activation after full verification")
    return result


def run(port, baseline, current, reviewed, snapshot_provider, output, *, full_bank=False, progress=None):
    baseline = TRANSPORT.pinned_baseline(baseline)
    expected = plan(baseline, current, full_bank=full_bank)
    require(COORDINATOR.canonical(reviewed) == COORDINATOR.canonical(expected), "Marker test plan differs")
    require(callable(snapshot_provider), "Full snapshot provider required")
    require(progress is None or callable(progress), "Optional progress callback must be callable")
    # Freeze every original payload/frame before creating mutation intent. The
    # unchanged-sector rewrite is confined to this explicit acceptance runner;
    # ordinary recovery's no-op/valid-marker refusals remain in place.
    prepared = []
    for offset in range(START, END if full_bank else START + SIZE, SIZE):
        payload = TRANSPORT.sector_payload(baseline, offset)
        frames = tuple(TRANSPORT.program_frame(i + 1, offset + i * 1024,
                                               payload[i * 1024:(i + 1) * 1024]) for i in range(4))
        prepared.append((offset, payload, frames))
    journal = COORDINATOR.Journal(output)
    transaction_id = str(uuid.uuid4())
    last_snapshot = None

    def snapshot():
        nonlocal last_snapshot
        data = snapshot_provider()
        require(isinstance(data, (bytes, bytearray)) and len(data) == len(current), "Full 8 MiB snapshot required")
        last_snapshot = bytes(data)
        return last_snapshot

    try:
        journal.append({"event": "transaction_start", "transaction_id": transaction_id, "plan": expected})
        require(snapshot() == current, "Fresh full snapshot differs before write-back test")
        for number, (offset, payload, frames) in enumerate(prepared, 1):
            def record(event, **fields):
                journal.append({"event": event, "offset": offset, **fields})

            require(TRANSPORT.read_sector(port, offset) == current[offset:offset + SIZE],
                    "Current sector changed before test")
            record("before_verified", sha256=digest(current[offset:offset + SIZE]))
            TRANSPORT.check_status(port, offset, offset + SIZE, "before_erase", record)
            TRANSPORT.command_ready(port)
            record("erase_intent", bytes=SIZE)
            READER.send(port, b"\x17" + offset.to_bytes(3, "little") + b"\x01\x00")
            READER.expect(port, READER.ACK)
            require(TRANSPORT.read_sector(port, offset) == b"\xff" * SIZE,
                    "Sector erase readback differs; no program attempted")
            record("erase_readback_verified")
            TRANSPORT.check_status(port, offset, offset + SIZE, "before_program", record)
            TRANSPORT.program_packets(port, frames, record)
            require(TRANSPORT.read_sector(port, offset) == payload, "Staged stock sector differs")
            record("marker_sector_staged" if offset == START else "stock_body_sector_verified",
                   sha256=digest(payload))
            if progress:
                progress(number, len(prepared))
        marker = TRANSPORT.commit_marker(port, baseline, current, snapshot, journal.append)
        require(last_snapshot == current and marker["snapshot_sha256"] == expected["proposed_snapshot_sha256"],
                "Final write-back test snapshot differs")
        journal.append({"event": "transaction_verified", "snapshot_sha256": digest(last_snapshot),
                        "sector_count": len(prepared), "marker_commit": True})
        report = {"status": "verified_via_supplied_transports", "transaction_id": transaction_id,
                  "snapshot_sha256": digest(last_snapshot), "marker_commit": marker,
                  "operation": expected["kind"], "rewritten_sector_count": len(prepared),
                  "physical_recovery_verified": False,
                  "scope": "Injected transport checks; physical acceptance requires external serial evidence"}
        journal.save_report(report)
        return report
    except (Exception, KeyboardInterrupt) as error:
        try:
            journal.append({"event": "transaction_stopped", "error_type": type(error).__name__,
                            "mutation_possible": journal.mutation_possible})
            journal.save_report({"status": "stopped", "transaction_id": transaction_id,
                                 "error_type": type(error).__name__, "mutation_possible": journal.mutation_possible})
        except Exception:
            pass
        raise
    finally:
        journal.close()
