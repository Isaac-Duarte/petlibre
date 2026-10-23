"""Mutation tests require the ignored, verified stock image; no device I/O."""

import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("ota_layout", ROOT / "src/tools/audit/audit-ota-layout.py")
ota = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ota)
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
LAYOUT = ROOT / "src/firmware/devices/plaf103/layout.json"


@evidence(STOCK.is_file(), "Requires verified stock OTA fixture")
class LayoutAuditTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not STOCK.is_file():
            raise unittest.SkipTest("Requires the private verified PLAF103 1.1.15 OTA image")
        cls.reference = STOCK.read_bytes()

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.image = Path(self.directory.name) / "candidate.bin"
        self.layout = Path(self.directory.name) / "layout.json"
        self.layout.write_bytes(LAYOUT.read_bytes())

    def audit_bytes(self, data, repair_checksum=False):
        data = bytearray(data)
        if repair_checksum:
            struct.pack_into("<I", data, 16, sum(data[32:]) & 0xFFFFFFFF)
        self.image.write_bytes(data)
        return ota.audit(self.image, STOCK, self.layout)

    def test_verified_reference_placements_and_zero_length_psram(self):
        report = self.audit_bytes(self.reference)
        self.assertEqual(report["candidate_placements"][1]["end_exclusive"], "0x002d6000")
        self.assertEqual(report["regions"][-1]["body_bytes"], 0)
        self.assertEqual(report["boot_acceptance"], "unproven")

    def test_payload_bit_flip_is_rejected(self):
        corrupt = bytearray(self.reference)
        corrupt[200] ^= 1
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.audit_bytes(corrupt)

    def test_declared_size_cannot_hide_truncation(self):
        with self.assertRaisesRegex(ValueError, "EOF"):
            self.audit_bytes(self.reference[:-1])

    def test_repaired_checksum_does_not_authorize_a_wrong_load_address(self):
        corrupt = bytearray(self.reference)
        struct.pack_into("<I", corrupt, 0x17020 + 12, 0x12345678)
        with self.assertRaisesRegex(ValueError, "km4_xip header/load address"):
            self.audit_bytes(corrupt, repair_checksum=True)

    def test_added_sector_cannot_exceed_the_reference_footprint(self):
        oversized = bytearray(self.reference + b"\xff" * 4096)
        struct.pack_into("<I", oversized, 20, len(oversized) - 32)
        with self.assertRaisesRegex(ValueError, "reference footprint"):
            self.audit_bytes(oversized, repair_checksum=True)

    def test_sdk_default_bank_address_is_rejected(self):
        layout = json.loads(self.layout.read_text())
        layout["ota_bank_offsets"][1] = "0x00106000"
        self.layout.write_text(json.dumps(layout))
        with self.assertRaisesRegex(ValueError, "stock table"):
            self.audit_bytes(self.reference)

    def test_preserve_range_overlap_is_rejected(self):
        layout = json.loads(self.layout.read_text())
        layout["known_preserve_ranges"].append({"name": "test_reserved",
            "start": "0x00007000", "end_exclusive": "0x00008000"})
        self.layout.write_text(json.dumps(layout))
        with self.assertRaisesRegex(ValueError, "test_reserved"):
            self.audit_bytes(self.reference)


if __name__ == "__main__":
    unittest.main()
