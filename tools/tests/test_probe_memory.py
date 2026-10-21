"""Startup mapping assumptions: pinned stock and candidate ROM, no live hardware."""

import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
ROM = ROOT / "private/firmware/plaf103-sdk-rom-inputs/km4-rom-text.bin"


@evidence(importlib.util.find_spec("unicorn") and STOCK.is_file() and ROM.is_file(),
                     "Requires private stock/SDK ROM fixtures and the probe environment")
class MemoryAssumptionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("probe_memory", ROOT / "tools/audit-probe-memory.py")
        cls.memory = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.memory)
        cls.stock = STOCK.read_bytes()
        cls.rom = ROM.read_bytes()

    def test_real_configuration_executes_for_both_cache_modes_and_reserved_entries(self):
        report = self.memory.audit()
        self.assertEqual(len(report["cases"]), 3)
        self.assertTrue(all(case["control"] == 5 and len(case["regions_added_by_application"]) == 5
                            for case in report["cases"]))
        self.assertEqual(report["cases"][2]["regions_added_by_application"][-1]["index"], 7)
        self.assertEqual(report["startup"]["first_task_control_literal"], 2)
        self.assertTrue(all(not value for value in report["limits"].values()))

    def test_candidate_rom_without_privileged_background_access_is_rejected(self):
        rom = bytearray(self.rom)
        offset = 0x1010CC1A - 0x1010A000
        self.assertEqual(rom[offset:offset + 4], bytes.fromhex("42f00402"))
        rom[offset + 2] = 0  # ORR r2,r2,#0 instead of setting PRIVDEFENA.
        with self.assertRaisesRegex(AssertionError, "PRIVDEFENA"):
            self.memory.execute_configuration(self.stock, bytes(rom), 1, 0)

    def test_changed_task_privilege_or_additional_control_write_requires_new_analysis(self):
        image = bytearray(self.stock)
        image[self.memory.file_offset(0x1000D5AC)] = 3
        with self.assertRaisesRegex(AssertionError, "first task"):
            self.memory.startup_evidence(bytes(image))
        image = bytearray(self.stock)
        offset = self.memory.file_offset(0x0E000200)
        image[offset:offset + 4] = bytes.fromhex("81f31488")
        with self.assertRaisesRegex(AssertionError, "CONTROL-write"):
            self.memory.startup_evidence(bytes(image))


if __name__ == "__main__":
    unittest.main()
