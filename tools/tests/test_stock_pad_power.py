"""Execute the actual stock shared-pad power sequence and a missing-enable mutation."""

import importlib.util
from pathlib import Path
import unittest
from support import evidence as requires_evidence

ROOT = Path(__file__).resolve().parents[2]
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"


@requires_evidence(importlib.util.find_spec("unicorn") and STOCK.is_file(),
                     "Requires private stock firmware and the probe environment")
class StockPadPowerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("pad_power", ROOT / "tools/emulate/trace-stock-pad-power.py")
        cls.tool = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.tool)

    def test_stock_supply_sequence_and_power_management_gates(self):
        report = self.tool.audit()
        self.assertEqual(len(report["cases"]), 5)
        self.assertTrue(all(not value for value in report["limits"].values()))

    def test_missing_pad_enable_is_distinct_from_powered_ldo(self):
        image = bytearray(STOCK.read_bytes())
        offset = 0x40 + 0x0C000336 - 0x0C000020
        self.assertEqual(image[offset:offset + 4], bytes.fromhex("40f20023"))
        image[offset + 3] = 0x03  # MOVW r3,#0 instead of #0x200.
        result = self.tool.run(image)
        control = int(result["final_control"], 16)
        self.assertEqual(control, 0x155)
        self.assertTrue(control & 1)  # LDO power alone does not set pad enable.
        self.assertFalse(control & (1 << 9))
        with self.assertRaisesRegex(ValueError, "pinned stock"):
            self.tool.audit(image)
