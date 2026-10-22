"""The FW10/FW11 marker sequence against the actual factory loader; no device I/O."""
import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("ota_markers", ROOT / "tools/emulate/emulate-ota-markers.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)
FM = TOOL.FM


@evidence(FM.FLASH.is_file() and FM.ROM.is_file() and FM.ORIGINAL.is_file(),
          "Requires private verified backup, SDK ROM and stock image")
class OtaMarkerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.flash, cls.rom = bytearray(FM.FLASH.read_bytes()), FM.ROM.read_bytes()
        cls.image = FM.ORIGINAL.read_bytes()

    def test_every_step_and_marker_cut_keeps_a_bootable_bank(self):
        results = TOOL.run(self.flash, self.rom, self.image)
        self.assertTrue(all(r["ok"] for r in results), [r for r in results if not r["ok"]])
        self.assertEqual(sum(r["state"].startswith("cut:") for r in results), 7)
        # Only the forbidden step (no recovery in bank 0) leaves nothing to boot.
        self.assertEqual([r["state"] for r in results if r["selected"] == "none"],
                         ["forbidden: give way with no recovery"])

    def test_an_uncleared_bank_1_still_wins(self):
        f = bytearray(self.flash)
        TOOL.place(f, 0, self.image)
        TOOL.place(f, 1, self.image)
        self.assertEqual(TOOL.selected(f, self.rom), "bank1")
        # Any single changed marker byte gives way to bank 0.
        for index in range(len(TOOL.MARKER)):
            marker = bytearray(TOOL.MARKER)
            marker[index] &= marker[index] - 1  # Clear the lowest set bit, as a NOR program can.
            TOOL.place(f, 1, self.image, bytes(marker))
            with self.subTest(index=index):
                self.assertEqual(TOOL.selected(f, self.rom), "bank0")


if __name__ == "__main__":
    unittest.main()
