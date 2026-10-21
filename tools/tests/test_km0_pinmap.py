"""Evidence-dependent checks for startup pulls and linked-object verification."""

import importlib.util
from pathlib import Path
import struct
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("km0_pinmap", ROOT / "firmware/tools/pinmap.py")
pinmap = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pinmap)
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"
SOURCE = (ROOT / "private/upstream/ameba-rtos-d/component/soc/realtek/amebad/"
          "fwlib/usrcfg/rtl8721dlp_pinmapcfg.c")


@evidence(STOCK.is_file() and SOURCE.is_file(),
                     "Requires ignored verified stock OTA and pinned SDK source")
class Km0PinmapTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.stock = STOCK.read_bytes()
        cls.source = SOURCE.read_text()

    def test_linked_stock_table_and_record_fields(self):
        report = pinmap.audit_linked_table(self.stock, pinmap.TABLE_ADDRESS, self.stock)
        self.assertEqual(report['rows'], 65)
        self.assertEqual(report['bytes'], 1300)
        self.assertTrue(report['matches_stock'])
        _, rows = pinmap.stock_table(self.stock)
        self.assertEqual(rows[13], (13, 0, 255, 255, 1))
        self.assertEqual(rows[33], (33, 0, 255, 255, 0))

    def test_overlay_changes_only_eight_active_fields(self):
        updated = pinmap.overlay(self.source, self.stock)
        changed = [(a, b) for a, b in zip(self.source.splitlines(), updated.splitlines()) if a != b]
        self.assertEqual(len(self.source.splitlines()), len(updated.splitlines()))
        self.assertEqual(len(changed), 8)
        for before, after in changed:
            self.assertEqual(after, before.replace('GPIO_PuPd_DOWN', 'GPIO_PuPd_NOPULL', 1)
                             if 'GPIO_PuPd_DOWN' in before else
                             before.replace('GPIO_PuPd_UP', 'GPIO_PuPd_NOPULL', 1))

    def test_unreviewed_sleep_field_is_rejected(self):
        lines = self.source.splitlines(keepends=True)
        index = next(i for i, s in enumerate(lines) if s.lstrip().startswith('{_PA_13,'))
        lines[index] = lines[index].replace('GPIO_PuPd_KEEP', 'GPIO_PuPd_DOWN', 1)
        with self.assertRaisesRegex(ValueError, 'sleep/low-power'):
            pinmap.overlay(''.join(lines), self.stock)

    def test_missing_or_duplicate_pins_are_rejected(self):
        for replacement in ('_PA_14,', '_PA_99,'):
            with self.subTest(replacement=replacement):
                with self.assertRaisesRegex(ValueError, 'pin order'):
                    pinmap.overlay(self.source.replace('_PA_13,', replacement), self.stock)

    def test_mutated_stock_reference_is_rejected(self):
        damaged = bytearray(self.stock)
        damaged[-1] ^= 1
        with self.assertRaisesRegex(ValueError, 'exact verified stock'):
            pinmap.overlay(self.source, damaged)
        with self.assertRaisesRegex(ValueError, 'exact verified stock'):
            pinmap.audit_linked_table(self.stock, pinmap.TABLE_ADDRESS, damaged)

    def test_stale_compiled_motor_pull_is_rejected(self):
        damaged = bytearray(self.stock)
        active_pull = 64 + pinmap.TABLE_ADDRESS - pinmap.XIP_ADDRESS + 13 * 20 + 4
        struct.pack_into('<I', damaged, active_pull, 1)
        # A valid container checksum must not hide stale or incorrect pad data.
        struct.pack_into('<I', damaged, 16, sum(damaged[32:]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(ValueError, 'differs from verified stock'):
            pinmap.audit_linked_table(damaged, pinmap.TABLE_ADDRESS, self.stock)

    def test_symbol_bounds_and_load_address_are_checked(self):
        with self.assertRaisesRegex(ValueError, 'outside the KM0 XIP body'):
            pinmap.audit_linked_table(self.stock, pinmap.XIP_ADDRESS - 4, self.stock)
        damaged = bytearray(self.stock)
        struct.pack_into('<I', damaged, 44, 0x0E000020)
        with self.assertRaisesRegex(ValueError, 'Unexpected candidate KM0 XIP header'):
            pinmap.audit_linked_table(damaged, pinmap.TABLE_ADDRESS, self.stock)


if __name__ == '__main__':
    unittest.main()
