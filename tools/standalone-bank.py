#!/usr/bin/env python3
"""Guarded pinned-diagnostic install/stock restore; injected transport only.

Separate from ordinary stock recovery. Only one reviewed diagnostic and bank 1
are accepted. A bootable source bank must match exactly before its marker sector
is erased first. No resume, retry, arbitrary images or automatic boot.
"""
import importlib.util
from pathlib import Path
import uuid

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("standalone_coordinator", ROOT / "tools/recovery-transaction.py")
C = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(C)
T, R = C.SECTORS, C.SECTORS.READER
require, digest, canonical = C.require, C.digest, C.canonical
START, END, SIZE = T.START, T.END, T.SECTOR
CANDIDATE_SHA = "6f97391a7afa43b512987ac73eb7120a09d57450928f8f653ea05a6ff9aa8c97"
CANDIDATE_BYTES = 106528
KINDS = {"install": "pinned_uart_diagnostic_install", "restore": "pinned_uart_diagnostic_stock_restore"}


def candidate_payload(candidate):
    require(isinstance(candidate, bytes) and len(candidate) == CANDIDATE_BYTES and
            digest(candidate) == CANDIDATE_SHA, "Exact reviewed UART-only diagnostic required")
    return candidate[32:]


def images(baseline, current, candidate, direction):
    baseline = T.pinned_baseline(baseline)
    payload = candidate_payload(candidate)
    require(direction in KINDS, "Explicit install or restore direction required")
    require(isinstance(current, bytes) and len(current) == len(baseline), "Exact current 8 MiB snapshot required")
    require(current[:0x6000] == baseline[:0x6000], "Factory bytes differ")
    require(current[0x6000:0x6008] != baseline[START:START + 8], "Other bank must remain invalid")
    custom_bank = payload + baseline[START + len(payload):END]
    before_bank = baseline[START:END] if direction == "install" else custom_bank
    after_bank = custom_bank if direction == "install" else baseline[START:END]
    require(current[START:END] == before_bank, "Current bank is not the exact required bootable source image")
    proposed = current[:START] + after_bank + current[END:]
    staged = proposed[:START] + b"\xff" * 8 + proposed[START + 8:]
    return proposed, staged


def plan(baseline, current, candidate, direction):
    proposed, staged = images(baseline, current, candidate, direction)
    target_end = START + len(candidate) - 32
    offsets = [o for o in range(START, target_end, SIZE) if current[o:o + SIZE] != staged[o:o + SIZE]]
    require(offsets == list(range(START, target_end, SIZE)) and len(offsets) == 26,
            "Pinned diagnostic must change exactly its 26-sector footprint")
    return {"schema_version": 1, "kind": KINDS[direction], "direction": direction,
            "baseline_sha256": digest(baseline), "candidate_sha256": CANDIDATE_SHA,
            "current_snapshot_sha256": digest(current), "staged_snapshot_sha256": digest(staged),
            "proposed_snapshot_sha256": digest(proposed),
            "target_range": {"start": START, "end_exclusive": target_end},
            "ordered_sector_payloads": [{"offset": o, "bytes": SIZE,
                "before_sha256": digest(current[o:o + SIZE]), "stage_sha256": digest(staged[o:o + SIZE]),
                "marker_withheld": o == START} for o in offsets],
            "sector_erases": len(offsets), "staging_packets": len(offsets) * 4,
            "commit_packets": 1, "additional_commit_erases": 0,
            "marker_commit_sha256": digest(proposed[START:START + 1024]),
            "preserve_current_ranges": [{"start": a, "end_exclusive": b, "sha256": digest(current[a:b])}
                                        for a, b in ((0, START), (target_end, len(current)))],
            "scope": "Exact pinned diagnostic and original stock only; marker erased first; no application boot"}


def run(port, baseline, current, candidate, direction, reviewed, snapshot_provider, output, progress=None):
    expected = plan(baseline, current, candidate, direction)
    require(canonical(reviewed) == canonical(expected), "Reviewed standalone plan differs")
    require(callable(snapshot_provider) and (progress is None or callable(progress)), "Snapshot/progress callback required")
    proposed, staged = images(baseline, current, candidate, direction)
    prepared = []
    for sector in expected["ordered_sector_payloads"]:
        offset = sector["offset"]
        payload = staged[offset:offset + SIZE]
        frames = tuple(T.program_frame(i + 1, offset + i * 1024, payload[i * 1024:(i + 1) * 1024])
                       for i in range(4))
        prepared.append((offset, payload, frames))
    commit = T.program_frame(1, START, proposed[START:START + 1024])
    journal = C.Journal(output)
    transaction_id = str(uuid.uuid4())

    def snapshot():
        data = snapshot_provider()
        require(isinstance(data, (bytes, bytearray)) and len(data) == len(current), "Exact full snapshot required")
        return bytes(data)

    try:
        journal.append({"event": "transaction_start", "transaction_id": transaction_id, "plan": expected})
        require(snapshot() == current, "Fresh full snapshot differs before any mutation")
        journal.append({"event": "initial_snapshot_verified", "sha256": digest(current)})
        for number, (offset, payload, frames) in enumerate(prepared, 1):
            def record(event, **fields):
                journal.append({"event": event, "offset": offset, **fields})

            require(T.read_sector(port, offset) == current[offset:offset + SIZE], "Sector preimage drift")
            record("before_verified", sha256=digest(current[offset:offset + SIZE]))
            T.check_status(port, offset, offset + SIZE, "before_erase", record)
            record("erase_intent", bytes=SIZE)
            # Journal fsync may span an idle-ready interval; synchronize last.
            T.command_ready(port)
            R.send(port, b"\x17" + offset.to_bytes(3, "little") + b"\x01\x00")
            R.expect(port, R.ACK)
            require(T.read_sector(port, offset) == b"\xff" * SIZE, "Erase readback differs")
            record("erase_readback_verified")
            T.check_status(port, offset, offset + SIZE, "before_program", record)
            T.program_packets(port, frames, record)
            require(T.read_sector(port, offset) == payload, "Staged sector readback differs")
            record("marker_sector_staged" if offset == START else "body_sector_verified", sha256=digest(payload))
            if progress:
                progress(number, len(prepared))
        require(snapshot() == staged, "Full staged image or preserved ranges differ; activation refused")
        require(T.read_sector(port, START) == staged[START:START + SIZE], "Marker sector changed before activation")
        journal.append({"event": "marker_preconditions_verified", "offset": START,
                        "staged_snapshot_sha256": digest(staged)})

        def record_commit(event, **fields):
            journal.append({"event": event, "offset": START, "phase": "marker_commit", **fields})

        T.check_status(port, START, START + 1024, "before_marker_commit", record_commit)
        record_commit("marker_commit_intent", proposed_snapshot_sha256=digest(proposed))
        T.program_packets(port, (commit,), record_commit)
        require(T.read_sector(port, START) == proposed[START:START + SIZE], "Committed marker sector differs")
        require(snapshot() == proposed, "Final full image or preserved ranges differ")
        journal.append({"event": "marker_commit_verified", "offset": START, "snapshot_sha256": digest(proposed)})
        journal.append({"event": "transaction_verified", "snapshot_sha256": digest(proposed),
                        "sector_count": len(prepared), "marker_commit": True})
        report = {"status": "verified_via_supplied_transports", "operation": KINDS[direction],
                  "transaction_id": transaction_id, "snapshot_sha256": digest(proposed),
                  "rewritten_sector_count": len(prepared), "additional_commit_erases": 0,
                  "physical_boot_verified": False, "scope": "Injected transport checks; supervised bench acceptance separate"}
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
