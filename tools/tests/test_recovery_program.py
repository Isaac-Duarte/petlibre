"""Actual helper program-wrapper instructions with modeled NOR; no serial."""
import importlib.util
from pathlib import Path
import unittest
from support import evidence
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("recovery_program", ROOT / "tools/audit-recovery-program.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


@evidence(TOOL.HELPER.exists(), "Requires private pinned vendor helper")
class RecoveryProgramTests(unittest.TestCase):
    def test_auto_erase_history_and_explicit_sector_erase(self):
        report = TOOL.audit()
        self.assertEqual([c["target_matches_payload"] for c in report["cases"]], [False, True, True, False, True])
        self.assertEqual([c["helper_auto_sector_erases"] for c in report["cases"]], [0, 0, 1, 0, 0])
        self.assertTrue(all(c["outside_target_unchanged"] for c in report["cases"]))
        self.assertTrue(all(c["eight_byte_program_calls"] == 512 for c in report["cases"]))

    def test_wrong_helper_provenance_refused(self):
        with self.assertRaisesRegex(ValueError, "Pinned"):
            TOOL.execute(b"wrong")

    def test_unreviewed_status_write_cannot_pass_execution(self):
        spec = importlib.util.spec_from_file_location("probe_patch", ROOT / "tools/prepare-c-probe.py")
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        helper = bytearray(TOOL.HELPER.read_bytes())
        helper[0x284:0x288] = builder.thumb_bl(0x82284, 0x2129)
        # Deliberately bypass provenance to exercise the instruction allowlist.
        with patch.object(TOOL, "HELPER_SHA", TOOL.digest(helper)):
            with self.assertRaisesRegex(ValueError, "Unexpected program-path instruction: 0x2128"):
                TOOL.execute(bytes(helper))


if __name__ == "__main__":
    unittest.main()
