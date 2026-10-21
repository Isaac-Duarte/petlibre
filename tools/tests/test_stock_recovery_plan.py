"""Recovery plan preserves current settings and never emits a flash operation."""
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from support import evidence
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("stock_recovery_plan", ROOT / "tools/plan-stock-recovery.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)
BASELINE = ROOT / "private/fixtures/baseline.bin"


@evidence(BASELINE.exists(), "Requires private verified baseline")
class StockRecoveryPlanTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline = BASELINE.read_bytes()

    def test_identical_snapshot_needs_no_erase(self):
        self.assertEqual(TOOL.plan(self.baseline, self.baseline)["ordered_sector_payloads"], [])

    def test_marker_repair_retains_newer_settings_and_failed_image(self):
        current = bytearray(self.baseline)
        current[TOOL.START] = 0
        current[0x7000] ^= 1  # A failed experimental bank is retained as evidence.
        for at in (0x3E8000, 0x400000, 0x7D0000, 0x7DA000, 0x600000):
            current[at] ^= 1  # Include an unclassified range as well as known settings.
        report = TOOL.plan(self.baseline, bytes(current))
        self.assertEqual([s["offset"] for s in report["ordered_sector_payloads"]], [TOOL.START])
        for area in report["preserve_current_ranges"]:
            a, b = area["start"], area["end_exclusive"]
            self.assertEqual(area["sha256"], TOOL.digest(current[a:b]))

    def test_payload_sectors_before_activation_and_no_partial_sector(self):
        current = bytearray(self.baseline)
        for at in (TOOL.START, TOOL.START + TOOL.SECTOR + 17, TOOL.END - 1):
            current[at] ^= 1
        report = TOOL.plan(self.baseline, bytes(current))
        self.assertEqual([s["offset"] for s in report["ordered_sector_payloads"]],
                         [TOOL.START + TOOL.SECTOR, TOOL.END - TOOL.SECTOR, TOOL.START])
        for sector in report["ordered_sector_payloads"]:
            self.assertEqual(sector["bytes"], 4096)
            self.assertEqual(sector["offset"] % 4096, 0)
            self.assertNotEqual(sector["before_sha256"], sector["after_sha256"])

    def test_changed_factory_truncated_or_wrong_baseline_refused(self):
        current = bytearray(self.baseline)
        current[0x720] ^= 1
        with self.assertRaisesRegex(ValueError, "Factory"):
            TOOL.plan(self.baseline, bytes(current))
        with self.assertRaisesRegex(ValueError, "8 MiB"):
            TOOL.plan(self.baseline, self.baseline[:-1])
        with self.assertRaisesRegex(ValueError, "baseline"):
            TOOL.plan(bytes(current), self.baseline)

    def test_valid_target_with_changed_body_is_not_rewritten_in_place(self):
        current = bytearray(self.baseline)
        current[TOOL.START + TOOL.SECTOR] ^= 1
        with self.assertRaisesRegex(ValueError, "still valid"):
            TOOL.plan(self.baseline, bytes(current))

    def test_cli_payloads_withhold_marker_until_separate_no_erase_commit(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "private/verification", prefix="plan-payload-test-") as temporary:
            root = Path(temporary)
            (root / "baseline.bin").write_bytes(self.baseline)
            current = bytearray(self.baseline)
            current[TOOL.START] = 0
            current[0x3E8000] ^= 1
            (root / "current.bin").write_bytes(current)
            argv = ["plan-stock-recovery.py", "--baseline", str(root / "baseline.bin"),
                    "--current", str(root / "current.bin"), "--output", str(root / "plan")]
            with patch.object(sys, "argv", argv), patch("sys.stdout", new_callable=io.StringIO):
                TOOL.main()
            report = json.loads((root / "plan/plan.json").read_text())
            self.assertEqual(report["schema_version"], 2)
            sector = report["ordered_sector_payloads"][0]
            staged = (root / "plan" / sector["payload"]).read_bytes()
            self.assertEqual(staged[:8], b"\xff" * 8)
            self.assertEqual(TOOL.digest(staged), sector["stage_sha256"])
            proposed = bytearray(current)
            proposed[TOOL.START:TOOL.START + 4096] = staged
            self.assertEqual(TOOL.digest(proposed), report["activation"]["staged_snapshot_sha256"])
            commit = (root / "plan" / report["activation"]["payload"]).read_bytes()
            self.assertEqual(len(commit), 1024)
            self.assertEqual(TOOL.digest(commit), report["activation"]["payload_sha256"])
            self.assertTrue(all(a & b == b for a, b in zip(staged[:1024], commit)))
            proposed[TOOL.START:TOOL.START + 1024] = commit
            self.assertEqual(TOOL.digest(proposed), report["proposed_snapshot_sha256"])
            self.assertEqual(proposed[:TOOL.START], current[:TOOL.START])
            self.assertEqual(proposed[TOOL.END:], current[TOOL.END:])


if __name__ == "__main__":
    unittest.main()
