"""Offline raw-read verification: all public fixtures are synthetic."""

import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("flash_backup", ROOT / "tools/audit/audit-flash-backup.py")
BACKUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BACKUP)
CAPACITY = 8 * 1024 * 1024  # Test fixture only, never a hardware-capacity assertion.


def synthetic():
    data = bytearray(b"\xff" * CAPACITY)
    prefix = b"synthetic-not-a-backup!!!"
    data[:len(prefix)] = prefix
    for offset in [0x6000, 0x186000]:
        data[offset:offset + 8] = struct.pack("<II", *BACKUP.OTA.IMAGE_MAGIC)
    data[0x3EA000:0x3EA010] = bytes(range(16))
    data[0x7D6000:0x7D6010] = bytes(range(16, 32))
    return data


class BackupTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.first = Path(self.directory.name) / "read-1.bin"
        self.second = Path(self.directory.name) / "read-2.bin"
        self.data = synthetic()
        self.first.write_bytes(self.data)
        self.second.write_bytes(self.data)

    def audit(self, **kwargs):
        return BACKUP.audit(self.first, self.second, CAPACITY, "research-feeder", **kwargs)

    def test_identical_full_reads_preserve_unknowns_and_do_not_claim_recovery(self):
        report = self.audit()
        self.assertTrue(report["comparison_passed"])
        self.assertTrue(report["checks"]["all_observed_preserve_areas_covered"])
        self.assertEqual(report["differences"]["different_bytes"], 0)
        self.assertTrue(all(not value for value in report["limits"].values()))
        self.assertTrue(all(area["identical"] for area in report["known_preserve_areas"]))
        self.assertTrue(all(bank["marker_matches_known_magic"] for bank in report["application_banks"]["first"]))
        self.assertTrue(all(bank["currently_active"] == "unknown" for bank in report["application_banks"]["first"]))
        self.assertIsNone(report["stock_reference_sha256"])
        self.assertEqual(self.first.read_bytes(), self.data)

    def test_factory_bank_and_unmapped_changes_are_all_reported_without_values(self):
        for offset in [7, 0x7000, 0x300000, 0x3EA003, 0x7D6001]:
            self.data[offset] ^= 1
        self.second.write_bytes(self.data)
        report = self.audit()
        self.assertFalse(report["comparison_passed"])
        self.assertEqual(report["differences"]["different_bytes"], 5)
        self.assertEqual(len(report["changed_sectors"]), 5)
        changed = {r["name"] for r in report["known_preserve_areas"] if not r["identical"]}
        self.assertEqual(changed, {"pre_application", "observed_settings_sectors", "observed_plan_record_sectors"})
        unknown = next(s for s in report["changed_sectors"] if s["start"] == 0x300000)
        self.assertEqual(unknown["known_preserve_areas"], [])
        self.assertNotIn("first_byte", json.dumps(report))

    def test_identical_partial_and_padded_uniform_reads_do_not_pass(self):
        self.first.write_bytes(self.data[:-4096])
        self.second.write_bytes(self.data[:-4096])
        report = self.audit()
        self.assertFalse(report["comparison_passed"])
        self.assertFalse(report["checks"]["lengths_match_supplied_capacity"])
        for value in [b"\x00", b"\xff"]:
            self.first.write_bytes(value * CAPACITY)
            self.second.write_bytes(value * CAPACITY)
            self.assertFalse(self.audit()["comparison_passed"])

    def test_alias_pattern_and_uncovered_areas_are_explicit(self):
        half = self.data[:CAPACITY // 2]
        self.first.write_bytes(half * 2)
        self.second.write_bytes(half * 2)
        report = self.audit()
        self.assertFalse(report["comparison_passed"])
        self.assertIn("identical_halves_require_alias_or_content_investigation", report["pattern_anomalies"]["first"])
        report = BACKUP.audit(self.first, self.second, CAPACITY // 2, "research-feeder")
        self.assertFalse(report["checks"]["lengths_match_supplied_capacity"])
        self.first.write_bytes(half)
        self.second.write_bytes(half)
        report = BACKUP.audit(self.first, self.second, len(half), "research-feeder")
        self.assertFalse(report["comparison_passed"])
        self.assertFalse(report["checks"]["all_observed_preserve_areas_covered"])
        self.assertTrue(any(not area["covered_in_both"] for area in report["known_preserve_areas"]))

    def test_same_file_hardlink_invalid_capacity_and_wrong_reference_are_refused(self):
        with self.assertRaisesRegex(ValueError, "same file"):
            BACKUP.audit(self.first, self.first, CAPACITY, "research-feeder")
        self.second.unlink()
        self.second.hardlink_to(self.first)
        with self.assertRaisesRegex(ValueError, "same file"):
            self.audit()
        self.second.unlink()
        self.second.write_bytes(self.data)
        for capacity in [0, -1, CAPACITY - 1, BACKUP.MAX_BYTES + 4096]:
            with self.assertRaisesRegex(ValueError, "Capacity"):
                BACKUP.audit(self.first, self.second, capacity, "research-feeder")
        with self.assertRaisesRegex(ValueError, "Reference is not"):
            self.audit(stock_path=self.first)

    def test_report_is_private_exclusive_and_cannot_replace_inputs_or_follow_symlinks(self):
        (ROOT / "private").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / "private") as directory:
            report_path = Path(directory) / "audit.json"
            report = self.audit()
            BACKUP.write_report(report_path, report, [self.first, self.second])
            self.assertEqual(report_path.stat().st_mode & 0o777, 0o600)
            with self.assertRaisesRegex(ValueError, "already exists"):
                BACKUP.write_report(report_path, report, [])
            link = Path(directory) / "symlink.json"
            target = Path(directory) / "absent.json"
            link.symlink_to(target)
            with self.assertRaisesRegex(ValueError, "already exists"):
                BACKUP.write_report(link, report, [])
            self.assertFalse(target.exists())
        with self.assertRaisesRegex(ValueError, "under ignored"):
            BACKUP.write_report(Path(self.directory.name) / "public.json", {}, [])

    def test_diff_ranges_are_bounded_and_include_missing_tail_bytes(self):
        first = bytes(1024)
        second = b"\x01\x00" * 512
        report = BACKUP.differences(first, second)
        self.assertEqual(report["different_bytes"], 512)
        self.assertEqual(report["total_runs"], 512)
        self.assertEqual(len(report["runs"]), BACKUP.MAX_RUNS)
        self.assertTrue(report["runs_truncated"])
        self.assertEqual(BACKUP.differences(b"abc", b"abcdef")["runs"], [{"start": 3, "end_exclusive": 6}])


@evidence((ROOT / "private/firmware/PLAF103_OTA_010115.bin").is_file(),
                     "Requires the private pinned original OTA")
class BankIdentificationTests(unittest.TestCase):
    def test_bank_body_is_identified_even_when_old_marker_is_invalid_but_not_after_other_changes(self):
        original = BACKUP.reference(ROOT / "private/firmware/PLAF103_OTA_010115.bin")
        data = synthetic()
        payload = original[32:]
        for start in [0x6000, 0x186000]:
            data[start:start + len(payload)] = payload
        data[0x186000:0x186004] = b"\0" * 4
        layout = json.loads(BACKUP.LAYOUT.read_text())
        result = BACKUP.banks(data, layout, original)
        self.assertTrue(result[0]["marker_matches_known_magic"])
        self.assertFalse(result[1]["marker_matches_known_magic"])
        self.assertTrue(all(r["image_after_marker_normalization"] == "original-1.1.15" for r in result))
        self.assertEqual(data[0x186000:0x186004], b"\0" * 4)
        data[0x186100] ^= 1
        self.assertEqual(BACKUP.banks(data, layout, original)[1]["image_after_marker_normalization"], "unrecognized")


if __name__ == "__main__":
    unittest.main()
