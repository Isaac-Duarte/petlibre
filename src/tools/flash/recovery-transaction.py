#!/usr/bin/env python3
"""Recovery plan coordinator with durable journal; no CLI or port opening.

Only tested with in-memory helper/snapshot peers. Physical identity, independent
entry, serial ownership and snapshot freshness must be established separately.
"""
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import uuid

ROOT = Path(__file__).resolve().parents[3]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "src" / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


PLAN = module("stock_recovery_plan", "flash/plan-stock-recovery.py")
SECTORS = module("recovery_sector_transport", "flash/recovery-sector-transport.py")


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def utc():
    return datetime.now(timezone.utc).isoformat()


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class Journal:
    def __init__(self, output):
        output = Path(output).absolute()
        require(not output.is_symlink() and output.resolve().is_relative_to((ROOT / "private").resolve()),
                "Journal output must be a new private directory")
        require(output.parent.is_dir(), "Journal parent must already exist")
        output.mkdir(exist_ok=False, mode=0o700)
        self.output = output
        self.stream = os.fdopen(os.open(output / "journal.jsonl", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600),
                                "wb", buffering=0)
        self.sequence, self.head = 0, "0" * 64
        self.failed = self.mutation_possible = False
        try:
            sync_directory(output)
            sync_directory(output.parent)
        except BaseException:
            self.stream.close()
            raise

    def append(self, event):
        require(not self.failed, "Journal failed previously; no append or replay")
        entry = {"sequence": self.sequence, "previous_sha256": self.head, "utc": utc(), "event": event}
        new_head = digest(canonical(entry))
        entry["sha256"] = new_head
        data = canonical(entry) + b"\n"
        try:
            require(self.stream.write(data) == len(data), "Partial journal write")
            os.fsync(self.stream.fileno())
        except BaseException:
            self.failed = True
            raise
        self.sequence += 1
        self.head = new_head
        if event.get("event") in ("erase_intent", "program_packet_intent", "marker_commit_intent"):
            self.mutation_possible = True

    def save_report(self, report):
        data = canonical({**report, "journal_head_sha256": self.head}) + b"\n"
        with os.fdopen(os.open(self.output / "report.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600),
                       "wb", buffering=0) as stream:
            require(stream.write(data) == len(data), "Partial report write")
            os.fsync(stream.fileno())
        sync_directory(self.output)

    def close(self):
        self.stream.close()


def inspect_journal(path):
    """Check accidental corruption/truncation; never resume or send commands."""
    with Path(path).open("rb") as stream:
        data = stream.read(4 * 1024 * 1024 + 1)
    return inspect_journal_data(data)


def inspect_journal_data(data):
    """Check an already captured byte snapshot without a second filesystem read."""
    require(len(data) <= 4 * 1024 * 1024 and data.endswith(b"\n"), "Oversized or incomplete journal line")
    head, entries = "0" * 64, []
    for sequence, line in enumerate(data.splitlines()):
        entry = json.loads(line)
        require(isinstance(entry, dict) and set(entry) == {"sequence", "previous_sha256", "utc", "event", "sha256"},
                "Unexpected journal fields")
        require(isinstance(entry["event"], dict) and isinstance(entry["event"].get("event"), str),
                "Unexpected journal event")
        require(type(entry["sequence"]) is int and entry["sequence"] == sequence
                and entry["previous_sha256"] == head, "Broken journal ordering/hash link")
        supplied = entry.pop("sha256")
        require(digest(canonical(entry)) == supplied, "Journal content hash mismatch")
        head = supplied
        entries.append(entry["event"])
    require(entries, "Empty journal")
    return {"records": len(entries), "head_sha256": head,
            "first_event": entries[0].get("event"),
            "transaction_id": entries[0].get("transaction_id"),
            "verified_terminal": entries[-1].get("event") == "transaction_verified",
            "terminal_event": entries[-1].get("event"),
            "terminal_snapshot_sha256": entries[-1].get("snapshot_sha256"),
            "mutation_intent_recorded": any(e.get("event") in ("erase_intent", "program_packet_intent", "marker_commit_intent")
                                            for e in entries),
            "erase_order": [e["offset"] for e in entries if e.get("event") == "erase_intent"]}


def validate_plan(baseline, current, manifest):
    expected = PLAN.plan(baseline, current)
    require(isinstance(manifest, dict) and isinstance(manifest.get("created_utc"), str), "Reviewed plan required")
    fields = lambda plan: {k: v for k, v in plan.items() if k != "created_utc"}
    require(canonical(fields(manifest)) == canonical(fields(expected)), "Plan differs from recomputed exact baseline/current plan")
    return expected


def run_transaction(port, baseline, current_snapshot, manifest, snapshot_provider, output):
    baseline, current = bytes(baseline), bytes(current_snapshot)
    manifest = json.loads(canonical(manifest))  # Freeze supplied mutable containers.
    expected = validate_plan(baseline, current, manifest)
    require(callable(snapshot_provider), "Full-snapshot provider required")
    journal = Journal(output)  # A previous output is terminal; there is no resume path.
    transaction_id = str(uuid.uuid4())
    results = []
    last_snapshot = None

    def snapshot():
        nonlocal last_snapshot
        value = snapshot_provider()
        require(isinstance(value, (bytes, bytearray)) and len(value) == PLAN.CAPACITY,
                "Snapshot provider must return exactly 8 MiB")
        last_snapshot = bytes(value)
        return last_snapshot

    try:
        journal.append({"event": "transaction_start", "transaction_id": transaction_id,
                        "baseline_sha256": digest(baseline), "current_sha256": digest(current),
                        "plan_sha256": digest(canonical(manifest))})
        require(snapshot() == current, "Fresh full snapshot differs from reviewed current image")
        journal.append({"event": "initial_snapshot_verified", "sha256": digest(current)})
        for sector in expected["ordered_sector_payloads"]:
            result = SECTORS.restore_sector(port, baseline, sector["offset"], sector["before_sha256"], journal.append)
            require(result["after_sha256"] == sector["stage_sha256"]
                    and result["program_packets"] == (4 if sector["stage_required"] else 0),
                    "Sector result differs from reviewed staging plan")
            results.append(result)
        marker = None
        if expected["ordered_sector_payloads"]:
            # The planner's invalid-marker condition ensures a changed marker
            # sector is included. The transport stages it with eight erased bytes.
            require(results[-1]["offset"] == PLAN.START and results[-1]["marker_withheld"],
                    "Marker sector was not staged last")
            marker = SECTORS.commit_marker(port, baseline, current, snapshot, journal.append)
        # commit_marker's last action already verified a full final snapshot.
        # No-op plans likewise perform no device operation after the initial
        # snapshot. Reuse that immutable result rather than re-reading 8 MiB
        # without any intervening mutation or application execution.
        after = last_snapshot
        require(after[:PLAN.START] == current[:PLAN.START] and after[PLAN.END:] == current[PLAN.END:],
                "Final preserved ranges differ from current snapshot")
        require(after[PLAN.START:PLAN.END] == baseline[PLAN.START:PLAN.END]
                and digest(after) == expected["proposed_snapshot_sha256"], "Final stock bank/full-image verification failed")
        journal.append({"event": "transaction_verified", "snapshot_sha256": digest(after),
                        "sector_count": len(results), "marker_commit": marker is not None})
        report = {"status": "verified_via_supplied_transports", "transaction_id": transaction_id,
                  "sectors": results, "marker_commit": marker, "snapshot_sha256": digest(after),
                  "physical_recovery_verified": False, "same_unit_identity_verified_by_component": False,
                  "scope": "Transport/snapshot callback checks only; physical evidence remains external"}
        journal.save_report(report)
        return report
    except (Exception, KeyboardInterrupt) as error:
        try:
            journal.append({"event": "transaction_stopped", "error_type": type(error).__name__,
                            "mutation_possible": journal.mutation_possible})
            journal.save_report({"status": "stopped", "transaction_id": transaction_id,
                                 "error_type": type(error).__name__, "mutation_possible": journal.mutation_possible})
        except Exception:
            pass  # A failed disk remains evidence to inspect, never a replay trigger.
        raise
    finally:
        journal.close()
