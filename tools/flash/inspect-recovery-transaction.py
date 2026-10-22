#!/usr/bin/env python3
"""Inspect existing private recovery receipts, read only; never resume a write.

Exit 0: consistent verified callback receipt. Exit 1: stopped or incomplete.
Exit 2: invalid/unreadable evidence. None of these establishes physical recovery.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import stat

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("recovery_transaction", ROOT / "tools/flash/recovery-transaction.py")
TRANSACTION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TRANSACTION)


def require(value, message):
    if not value:
        raise ValueError(message)


def read_snapshot(path, directory, limit):
    path = path.resolve(strict=True)
    require(path.is_relative_to(directory), "Receipt file escapes its private directory")
    info = path.stat()
    require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= limit, "Receipt must be a bounded regular file")
    with path.open("rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= limit, "Receipt must be a bounded regular file")
        data = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
    identity = lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)
    require(identity(before) == identity(after) == identity(path.stat()) and len(data) == before.st_size,
            "Receipt changed during inspection")
    return data


def inspect(directory):
    directory = Path(directory).resolve(strict=True)
    require(directory.is_dir() and directory.is_relative_to((ROOT / "private").resolve()),
            "Receipt directory must be under private")
    journal = TRANSACTION.inspect_journal_data(read_snapshot(directory / "journal.jsonl", directory, 4 * 1024 * 1024))
    require(journal["first_event"] == "transaction_start", "Journal lacks transaction start")
    result = {"receipt_status": "incomplete", "journal_records": journal["records"],
              "journal_head_sha256": journal["head_sha256"],
              "recorded_erase_intents": len(journal["erase_order"]),
              "mutation_intent_recorded": journal["mutation_intent_recorded"],
              "physical_recovery_verified": False,
              "scope": "Recorded callback results only; identity, freshness and physical write acceptance not established"}
    report_path = directory / "report.json"
    if not report_path.exists():
        result["reason"] = "Report missing; terminal journal alone is insufficient"
        return result, 1
    report = json.loads(read_snapshot(report_path, directory, 1024 * 1024))
    require(isinstance(report, dict), "Report must be an object")
    require(report.get("journal_head_sha256") == journal["head_sha256"], "Report/journal heads differ")
    require(report.get("transaction_id") == journal["transaction_id"] and isinstance(journal["transaction_id"], str),
            "Report/journal transaction IDs differ")
    require(report.get("physical_recovery_verified", False) is False
            and report.get("same_unit_identity_verified_by_component", False) is False,
            "Receipt claims physical evidence beyond this component's scope")
    if report.get("status") == "verified_via_supplied_transports":
        require(journal["verified_terminal"], "Verified report lacks terminal verification record")
        snapshot_sha = report.get("snapshot_sha256")
        require(isinstance(snapshot_sha, str) and re.fullmatch(r"[0-9a-f]{64}", snapshot_sha)
                and snapshot_sha == journal["terminal_snapshot_sha256"], "Report/journal final snapshot hashes differ")
        result.update(receipt_status="verified_callback_record", snapshot_sha256=snapshot_sha)
        return result, 0
    require(report.get("status") == "stopped" and journal["terminal_event"] == "transaction_stopped",
            "Report/terminal state mismatch")
    require(type(report.get("mutation_possible")) is bool
            and report["mutation_possible"] == journal["mutation_intent_recorded"],
            "Stopped report/recorded mutation uncertainty mismatch")
    result.update(receipt_status="stopped", mutation_possible=report["mutation_possible"])
    return result, 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    try:
        result, code = inspect(args.directory)
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(json.dumps({"receipt_status": "invalid", "error_type": type(error).__name__,
                          "error": str(error), "physical_recovery_verified": False}))
        return 2
    print(json.dumps(result, indent=2))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
