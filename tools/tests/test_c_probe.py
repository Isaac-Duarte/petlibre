"""Exact-patch gates and real ARM execution/mutation checks for the C probe."""

import importlib.util
from pathlib import Path
import struct
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("c_probe_patch", ROOT / "tools/prepare-c-probe.py")
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)
DIRECTORY = ROOT / "private/firmware/experiments/rust-probe-1.1.17"


@evidence(PATCH.STOCK.is_file() and (DIRECTORY / "probe.bin").is_file(),
          "Requires verified stock and compiled probe fixtures")
class Fixtures:
    @classmethod
    def setUpClass(cls):
        if not PATCH.STOCK.is_file() or not (DIRECTORY / "probe.bin").is_file():
            raise unittest.SkipTest("Requires the private stock and compiled probe fixtures")
        cls.original = PATCH.STOCK.read_bytes()
        cls.code = (DIRECTORY / "probe.bin").read_bytes()


class PatchTests(Fixtures, unittest.TestCase):
    def test_exact_fixed_patch_preserves_load_headers_and_erase_footprint(self):
        candidate = PATCH.apply_patch(self.original, self.code)
        report = PATCH.verify_patch(self.original, candidate, self.code)
        self.assertTrue(report["same_load_headers"])
        self.assertEqual(len(candidate), len(self.original))
        self.assertEqual(candidate[PATCH.SLOT_OFFSET:PATCH.SLOT_OFFSET + len(self.code)], self.code)
        self.assertEqual(candidate[PATCH.VERSION_OFFSET:PATCH.VERSION_OFFSET + 7], b"1.1.17\0")

    def test_repaired_checksum_does_not_hide_unreviewed_code_or_header_changes(self):
        for offset in [0x17040, 0x17020 + 8, PATCH.HOOK_OFFSET, PATCH.SLOT_OFFSET + 400]:
            candidate = bytearray(PATCH.apply_patch(self.original, self.code))
            candidate[offset] ^= 1
            struct.pack_into("<I", candidate, 16, sum(candidate[32:]) & 0xFFFFFFFF)
            with self.assertRaisesRegex(ValueError, "Unexpected change"):
                PATCH.verify_patch(self.original, candidate, self.code)

    def test_oversized_empty_and_wrong_reference_inputs_are_refused(self):
        for code in [b"", b"\x00" * 513]:
            with self.assertRaisesRegex(ValueError, "does not fit"):
                PATCH.apply_patch(self.original, code)
        changed = bytearray(self.original)
        changed[0x17040] ^= 1
        with self.assertRaisesRegex(ValueError, "Not the verified"):
            PATCH.apply_patch(changed, self.code)

    def test_branch_encoder_matches_stock_instruction_and_refuses_invalid_reach(self):
        self.assertEqual(PATCH.thumb_bl(PATCH.HOOK_ADDRESS, 0x0E00BC71), PATCH.HOOK_ORIGINAL)
        self.assertEqual(PATCH.thumb_bl(0x1000, 0x1004), bytes.fromhex("00f000f8"))
        for source, target in [(0x1001, 0x1004), (0x1000, 0x2001004), (0x2001000, 0x1000)]:
            with self.assertRaises(ValueError):
                PATCH.thumb_bl(source, target)


@evidence(importlib.util.find_spec("unicorn") and importlib.util.find_spec("capstone"),
                     "Run with the private probe environment")
class ExecutionTests(Fixtures, unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        spec = importlib.util.spec_from_file_location("c_probe_emulator", ROOT / "tools/emulate-c-probe.py")
        cls.emulator = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.emulator)

    def test_complete_handler_boundary_and_allocation_failure_cases(self):
        report = self.emulator.verify(DIRECTORY)
        self.assertEqual(len(report["cases"]), 9)
        self.assertEqual(report["topic_contract"]["reply_suffix"], "config/post")
        self.assertNotEqual(report["cases"][0]["fields"]["localReProbe"],
                            report["cases"][1]["fields"]["localReProbe"])
        self.assertEqual(self.emulator.proof("hello"), "c1-4f9f2cab")
        self.assertEqual(self.emulator.proof("a"), "c1-e40c292c")

    def test_changed_hash_constant_and_bypassed_hook_cannot_pass_execution_proof(self):
        candidate = bytearray(PATCH.apply_patch(self.original, self.code))
        baseline = self.emulator.execute(self.original, "probe-0001")
        # Keep a valid Thumb program while changing its computed result.
        prime = self.code.index(struct.pack("<I", 16777619))
        candidate[PATCH.SLOT_OFFSET + prime] ^= 2
        result = self.emulator.execute(bytes(candidate), "probe-0001")
        with self.assertRaisesRegex(AssertionError, "incorrect proof"):
            self.emulator.verify_reply(baseline, result, "probe-0001")
        candidate = bytearray(PATCH.apply_patch(self.original, self.code))
        candidate[PATCH.HOOK_OFFSET:PATCH.HOOK_OFFSET + 4] = PATCH.HOOK_ORIGINAL
        result = self.emulator.execute(bytes(candidate), "probe-0001")
        with self.assertRaises(AssertionError):
            self.emulator.verify_reply(baseline, result, "probe-0001")

    def test_own_code_cannot_write_into_the_json_object(self):
        candidate = bytearray(PATCH.apply_patch(self.original, self.code))
        # STR r0,[r4] would write into the caller's JSON object instead of stack.
        candidate[PATCH.SLOT_OFFSET:PATCH.SLOT_OFFSET + 2] = bytes.fromhex("2060")
        with self.assertRaisesRegex(AssertionError, "outside its stack"):
            self.emulator.execute(bytes(candidate), "probe-0001")


if __name__ == "__main__":
    unittest.main()
