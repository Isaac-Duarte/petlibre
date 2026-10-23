"""Offline boundary and semantic checks for the candidate ROM recovery audit."""
import importlib.util
from pathlib import Path
import struct
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("recovery_boot", ROOT / "src/tools/audit/audit-recovery-boot.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)
ROM = ROOT / "private/upstream/ameba-rtos-d/project/realtek_amebaD_va0_example/GCC-RELEASE/project_lp/asdk/lib/rom_bcut/km0_irom.bin"


def fixture():
    data = bytearray(b"\xff" * TOOL.FLASH_BYTES)
    for name, offset, length, address in TOOL.SEGMENTS:
        struct.pack_into("<8I", data, offset,
                         *(TOOL.MAGIC + (length, address) + (0xFFFFFFFF,) * 4))
        if name.endswith("ram"):
            struct.pack_into("<8I", data, offset + 32, address + 33, *([0] * 7))
    return data


class RecoveryBootTests(unittest.TestCase):
    def test_factory_ranges_and_entry_bounds(self):
        data = fixture()
        segments = TOOL.factory_segments(data)
        self.assertEqual([s["body_bytes"] for s in segments], [0x6E0, 0x910, 0, 0x1030])
        struct.pack_into("<I", data, 0x720, 0x83001)
        with self.assertRaisesRegex(ValueError, "escapes"):
            TOOL.factory_segments(data)

    def test_truncated_or_changed_header_rejected(self):
        data = fixture()
        with self.assertRaisesRegex(ValueError, "8 MiB"):
            TOOL.factory_segments(data[:-1])
        data[0x4010] = 0
        with self.assertRaisesRegex(ValueError, "header"):
            TOOL.factory_segments(data)

    def test_wrong_dump_rejected_before_emulation(self):
        with self.assertRaisesRegex(ValueError, "verified research-unit"):
            TOOL.audit(b"wrong", b"wrong")

    @evidence(ROM.exists(), "Private pinned SDK ROM unavailable")
    def test_actual_candidate_selector_eight_states(self):
        rom = ROM.read_bytes()
        self.assertEqual(TOOL.digest(rom), TOOL.ROM_SHA256)
        cases = TOOL.selection_cases(rom)
        self.assertEqual(len(cases), 8)
        self.assertEqual(sum(c["route"] == "uart_download" for c in cases), 6)

    @evidence(ROM.exists(), "Private pinned SDK ROM unavailable")
    def test_semantic_mutations_change_selector(self):
        original = ROM.read_bytes()
        for offset, bit in ((0x50D2, 0x40), (0x4785, 0x10)):
            with self.subTest(offset=hex(offset)):
                rom = bytearray(original)
                rom[offset] ^= bit
                with self.assertRaises(ValueError):
                    TOOL.selection_cases(bytes(rom))


if __name__ == "__main__":
    unittest.main()
