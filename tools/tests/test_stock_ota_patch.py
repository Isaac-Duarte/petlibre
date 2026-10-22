"""Check the fixed experimental patch against the actual pinned stock image."""

import importlib.util
from pathlib import Path
import struct
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("stock_patch", ROOT / "tools/flash/prepare-stock-ota-test.py")
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)


@evidence(PATCH.STOCK.is_file(), "Requires verified stock OTA fixture")
class StockPatchTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not PATCH.STOCK.is_file():
            raise unittest.SkipTest("Requires the ignored verified stock image")
        cls.original = PATCH.STOCK.read_bytes()

    def candidate(self):
        result = bytearray(self.original)
        result[0xEDC9A] = ord("6")
        struct.pack_into("<I", result, 16, sum(result[32:]) & 0xFFFFFFFF)
        return result

    def test_one_payload_byte_and_valid_checksum_preserve_image_bodies(self):
        candidate = self.candidate()
        changes = PATCH.verify_patch(self.original, candidate)
        payload_changes = [entry for entry in changes if int(entry["file_offset"], 16) >= 32]
        self.assertEqual(payload_changes, [{"file_offset": "0x000edc9a", "before": 53, "after": 54}])
        self.assertEqual(candidate[:16], self.original[:16])
        self.assertEqual(candidate[20:32], self.original[20:32])
        self.assertEqual(candidate[32:0xEDC9A], self.original[32:0xEDC9A])
        self.assertEqual(candidate[0xEDC9B:], self.original[0xEDC9B:])

    def test_repaired_checksum_cannot_hide_an_additional_instruction_change(self):
        candidate = self.candidate()
        candidate[0x17040] ^= 1
        struct.pack_into("<I", candidate, 16, sum(candidate[32:]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(ValueError, "Unexpected changed byte"):
            PATCH.verify_patch(self.original, candidate)

    def test_wrong_reference_is_refused_even_with_a_valid_container_checksum(self):
        original = bytearray(self.original)
        original[0x17040] ^= 1
        struct.pack_into("<I", original, 16, sum(original[32:]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(ValueError, "Not the verified stock"):
            PATCH.verify_patch(original, self.candidate())

    def test_checksum_damage_and_truncation_are_refused(self):
        candidate = self.candidate()
        candidate[16] ^= 1
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            PATCH.verify_patch(self.original, candidate)
        with self.assertRaisesRegex(ValueError, "size changed"):
            PATCH.verify_patch(self.original, self.candidate()[:-1])


if __name__ == "__main__":
    unittest.main()
