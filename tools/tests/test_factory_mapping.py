"""Actual factory-loader mapping and bank-selection fixtures; no device I/O."""
import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("factory_mapping", ROOT / "tools/audit/audit-factory-mapping.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


@evidence(TOOL.FLASH.is_file() and TOOL.ROM.is_file() and
                     TOOL.PROBE.is_file() and TOOL.ORIGINAL.is_file(),
                     "Requires private verified backup, SDK ROM and probe")
class FactoryMappingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.flash, cls.rom = TOOL.FLASH.read_bytes(), TOOL.ROM.read_bytes()
        cls.stock, cls.probe = TOOL.ORIGINAL.read_bytes(), TOOL.PROBE.read_bytes()

    def test_actual_backup_and_both_image_banks(self):
        report = TOOL.audit(self.flash, self.rom, self.stock, self.probe)
        self.assertEqual(report["actual_backup_selection"]["result"], 2)
        self.assertEqual(report["fallback_selection_fixture"]["result"], 1)
        self.assertEqual(report["both_valid_fixture"]["result"], 2)
        self.assertEqual(len(report["stock_and_probe_mapping_fixtures"]), 4)
        for fixture in report["stock_and_probe_mapping_fixtures"]:
            self.assertEqual(fixture["slot_ota_file_offset"], "0x14fca0")
            self.assertEqual(fixture["mapping"]["configs"][-1]["end"], 0x0E138FFF)

    def test_changed_rounding_instruction_cannot_pass(self):
        changed = bytearray(self.flash)
        changed[0x96] ^= 0x40  # LSLS #12 -> #13 changes the final extent.
        with self.assertRaises(ValueError):
            TOOL.check_mapping(TOOL.execute(changed, self.rom, 1), self.stock, 1)

    def test_mismatched_km4_signature_stops_at_warning_loop(self):
        changed = bytearray(self.flash)
        changed[0x186000 + 0x17000] ^= 1
        result = TOOL.execute(changed, self.rom, 1)
        self.assertEqual(result["stop"], "0x8000182")
        with self.assertRaisesRegex(ValueError, "Valid bank"):
            TOOL.check_mapping(result, self.stock, 1)

    def test_changed_provenance_rejected_before_execution(self):
        with self.assertRaisesRegex(ValueError, "stock hash"):
            TOOL.audit(self.flash, self.rom, b"wrong", self.probe)


if __name__ == "__main__":
    unittest.main()
