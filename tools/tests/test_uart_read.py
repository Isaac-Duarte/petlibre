"""Vendor helper execution with synthetic flash/UART; never touches hardware."""

import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin"


@evidence(importlib.util.find_spec("unicorn") and HELPER.is_file(),
                     "Requires the private vendor helper and probe environment")
class ReadHelperTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("uart_read", ROOT / "tools/emulate-uart-read.py")
        cls.model = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.model)
        cls.helper = HELPER.read_bytes()

    def test_real_helper_startup_and_read_cover_setup_branches_and_packet_retry(self):
        report = self.model.audit()
        self.assertEqual(len(report["cases"]), 4)
        self.assertEqual({case["received_sha256"] for case in report["cases"]},
                         {"70b7f49fe9bc453cf62963aa8a01b243fbc90c926dba37c50f9dcc2e6d8873bf"})
        self.assertEqual(report["cases"][-1]["packet_attempts"], 5)
        self.assertTrue(all(case["accepted_packets"] == 4 and case["synthetic_flash_unchanged"]
                            for case in report["cases"]))
        self.assertTrue(all(not value for value in report["limits"].values()))
        self.assertTrue(all(case["observed_setup_requests"][0] == {"name":"RSIP_OTF_Cmd","enable":False}
                            for case in report["cases"]))

    def test_mutated_helper_cannot_call_flash_erase(self):
        helper = bytearray(self.helper)
        helper[0x20:0x24] = self.model.PATCH.thumb_bl(0x82020, 0x2147)
        with self.assertRaisesRegex(AssertionError, "forbidden flash mutation"):
            self.model.execute(bytes(helper))

    def test_mutated_helper_cannot_directly_store_to_mapped_flash(self):
        helper = bytearray(self.helper)
        helper[0x20:0x22] = bytes.fromhex("0860")  # STR r0,[r1], fixture r1 points into protected flash.
        with self.assertRaises(self.model.UcError):
            self.model.execute(bytes(helper))


if __name__ == "__main__":
    unittest.main()
