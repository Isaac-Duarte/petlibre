"""Read-only receipt CLI distinguishes verified callback evidence from recovery."""
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("receipt", ROOT / "tools/inspect-recovery-transaction.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


class RecoveryReceiptTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "private/verification").mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT / "private/verification", prefix="receipt-test-")
        self.parent = Path(self.temporary.name)
        self.addCleanup(self.temporary.cleanup)

    def receipt(self, name="run", state="verified"):
        output = self.parent / name
        journal = TOOL.TRANSACTION.Journal(output)
        journal.append({"event": "transaction_start", "transaction_id": "synthetic-test-transaction"})
        if state == "incomplete":
            journal.close()
            return output
        if state == "verified":
            journal.append({"event": "transaction_verified", "snapshot_sha256": "a" * 64})
            report = {"status": "verified_via_supplied_transports", "snapshot_sha256": "a" * 64,
                      "physical_recovery_verified": False, "same_unit_identity_verified_by_component": False}
        else:
            journal.append({"event": "erase_intent", "offset": 0x186000})
            journal.append({"event": "transaction_stopped", "mutation_possible": True})
            report = {"status": "stopped", "mutation_possible": True}
        journal.save_report({"transaction_id": "synthetic-test-transaction", **report})
        journal.close()
        return output

    def cli(self, directory):
        output = io.StringIO()
        with patch.object(sys, "argv", ["inspect-recovery-transaction.py", str(directory)]), \
                patch("sys.stdout", output):
            code = TOOL.main()
        return json.loads(output.getvalue()), code

    def test_verified_receipt_exit_zero_never_claims_physical_recovery(self):
        directory = self.receipt()
        before = {p.name: p.read_bytes() for p in directory.iterdir()}
        result, code = self.cli(directory)
        self.assertEqual(code, 0)
        self.assertEqual(result["receipt_status"], "verified_callback_record")
        self.assertFalse(result["physical_recovery_verified"])
        self.assertEqual(before, {p.name: p.read_bytes() for p in directory.iterdir()})

    def test_stopped_and_missing_report_exit_one_with_no_resume_action(self):
        stopped, code = self.cli(self.receipt("stopped", "stopped"))
        self.assertEqual(code, 1)
        self.assertEqual(stopped["receipt_status"], "stopped")
        self.assertTrue(stopped["mutation_possible"])
        incomplete, code = self.cli(self.receipt("incomplete", "incomplete"))
        self.assertEqual(code, 1)
        self.assertEqual(incomplete["receipt_status"], "incomplete")
        directory = self.receipt("terminal-without-report")
        (directory / "report.json").unlink()
        self.assertEqual(self.cli(directory)[1], 1)

    def test_crosspaired_or_overclaimed_report_is_invalid(self):
        directory = self.receipt()
        path = directory / "report.json"
        original = json.loads(path.read_text())
        for key, value in (("journal_head_sha256", "0" * 64), ("snapshot_sha256", "b" * 64),
                            ("transaction_id", "another-transaction"), ("physical_recovery_verified", True),
                            ("status", "stopped")):
            with self.subTest(key=key):
                path.write_text(json.dumps({**original, key: value}))
                self.assertEqual(self.cli(directory)[1], 2)

    def test_truncated_or_changed_journal_exit_two_without_repair(self):
        directory = self.receipt()
        path = directory / "journal.jsonl"
        original = path.read_bytes()
        for data in (original[:-1], original.replace(b"transaction_verified", b"transaction_modified")):
            path.write_bytes(data)
            result, code = self.cli(directory)
            self.assertEqual(code, 2)
            self.assertEqual(result["receipt_status"], "invalid")
            self.assertEqual(path.read_bytes(), data)

    def test_symlink_escape_and_nonregular_journal_are_rejected_before_read(self):
        directory = self.receipt()
        path = directory / "journal.jsonl"
        path.unlink()
        foreign = self.parent / "outside-journal"
        foreign.write_text("never read outside receipt directory")
        path.symlink_to(foreign)
        self.assertEqual(self.cli(directory)[1], 2)
        path.unlink()
        os.mkfifo(path)
        self.assertEqual(self.cli(directory)[1], 2)


if __name__ == "__main__":
    unittest.main()
