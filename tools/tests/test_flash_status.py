"""GD25Q64E primary-table fixtures and actual helper status-read dispatch."""
import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


TOOL = module("flash_status", "flash/flash-status.py")
SIM = module("status_peer", "emulate/emulate-recovery-session.py")


class FlashStatusTests(unittest.TestCase):
    def test_all_bp_cmp_combinations_against_datasheet_ranges(self):
        # Table 5, GD25Q64E Rev1.6 pp12–13, endpoints converted to exclusive.
        cmp0 = [None, (0x7E0000, 0x800000), (0x7C0000, 0x800000), (0x780000, 0x800000),
                (0x700000, 0x800000), (0x600000, 0x800000), (0x400000, 0x800000), (0, 0x800000),
                None, (0, 0x20000), (0, 0x40000), (0, 0x80000), (0, 0x100000), (0, 0x200000),
                (0, 0x400000), (0, 0x800000),
                None, (0x7FF000, 0x800000), (0x7FE000, 0x800000), (0x7FC000, 0x800000),
                (0x7F8000, 0x800000), (0x7F8000, 0x800000), (0x7F8000, 0x800000), (0, 0x800000),
                None, (0, 0x1000), (0, 0x2000), (0, 0x4000), (0, 0x8000), (0, 0x8000),
                (0, 0x8000), (0, 0x800000)]
        # Table 6's complement map is checked through coverage, not shared decoder logic.
        for bp, expected in enumerate(cmp0):
            for complement in (0, 1):
                with self.subTest(bp=bp, cmp=complement):
                    ranges = TOOL.decode(bp << 2, complement << 6, 0)["protected_ranges"]
                    boundaries = sorted({0, 0x800000, *(expected or [])})
                    for a, b in zip(boundaries, boundaries[1:]):
                        protected = any(r["start"] <= a and b <= r["end_exclusive"] for r in ranges)
                        base_protected = expected is not None and expected[0] <= a and b <= expected[1]
                        self.assertEqual(protected, base_protected != bool(complement))

    def test_bank_overlap_and_exact_boundary_are_part_specific(self):
        self.assertTrue(TOOL.assess_range(0x14, 2, 0x20, 0x186000, 0x2D6000)["software_preconditions_met"])
        blocked = TOOL.assess_range(0x34, 2, 0x20, 0x186000, 0x2D6000)
        self.assertEqual(blocked["protected_overlap"], [{"start": 0x186000, "end_exclusive": 0x200000}])
        self.assertTrue(TOOL.assess_range(0x34, 2, 0x20, 0x200000, 0x2D6000)["software_preconditions_met"])
        self.assertFalse(TOOL.assess_range(0x64, 0x40, 0, 0x186000, 0x2D6000)["software_preconditions_met"])

    def test_busy_suspend_latch_and_status_locks_have_distinct_meanings(self):
        for sr1, sr2 in ((1, 0), (0, 128), (0, 4)):
            self.assertFalse(TOOL.assess_range(sr1, sr2, 0, 0x186000, 0x187000)["software_preconditions_met"])
        for sr1, sr2 in ((0, 0), (2, 2), (0x80, 1)):
            result = TOOL.assess_range(sr1, sr2, 0x20, 0x186000, 0x187000)
            self.assertTrue(result["software_preconditions_met"])
            self.assertFalse(result["physical_write_permission_established"])
        decoded = TOOL.decode(0x80, 0x3B, 0x61)
        self.assertEqual(decoded["security_register_locks"], [True, True, True])
        self.assertEqual(decoded["srp1_srp0"], 3)
        self.assertTrue(decoded["quad_enable"])
        self.assertEqual(decoded["driver_strength_field"], 3)

    def test_invalid_bytes_or_ranges_rejected(self):
        for sr1, sr2, sr3 in ((-1, 0, 0), (0, 256, 0), (False, 0, 0), (0, 0, "0")):
            with self.assertRaises(ValueError):
                TOOL.decode(sr1, sr2, sr3)
        for a, b in ((0, 0), (-1, 4096), (0, 0x800001), (False, 4096)):
            with self.assertRaises(ValueError):
                TOOL.assess_range(0, 0, 0, a, b)


@evidence(SIM.HELPER.exists(), "Requires private pinned helper")
class FlashStatusTransportTests(unittest.TestCase):
    def test_actual_helper_reads_only_three_status_commands_and_preserves_flash(self):
        for supplied in ((0x14, 2, 0x20), (0x21, 0x15, 0x06)):
            with self.subTest(supplied=supplied):
                peer = SIM.EmulatedHelperPort(SIM.HELPER.read_bytes(), status_registers=supplied)
                self.assertEqual(TOOL.read_status_registers(peer), supplied)
                self.assertEqual(bytes(peer.host_tx), bytes.fromhex("210501213501211501"))
                self.assertEqual([c["opcode"] for c in peer.calls], [5, 53, 21])
                self.assertTrue(all(c["name"] == "read_status" and c["bytes"] == 1 for c in peer.calls))
                self.assertEqual(peer.snapshot(), peer.before)

    def test_wrong_echo_or_lost_response_stops_without_retry(self):
        for fault in ("wrong_echo", "lost_response"):
            class Peer(SIM.EmulatedHelperPort):
                def read(self, count):
                    data = super().read(count)
                    if self.calls and data == b"\x21":
                        if fault == "lost_response":
                            raise TimeoutError("Synthetic missing status response")
                        return b"\x06"
                    return data

            with self.subTest(fault=fault):
                peer = Peer(SIM.HELPER.read_bytes(), status_registers=(0, 2, 0x20))
                with self.assertRaises((ValueError, TimeoutError)):
                    TOOL.read_status_registers(peer)
                self.assertEqual(bytes(peer.host_tx), bytes.fromhex("210501"))
                self.assertEqual(peer.snapshot(), peer.before)

    def test_status_write_command_reaches_no_modeled_mutation_api(self):
        peer = SIM.EmulatedHelperPort(SIM.HELPER.read_bytes(), status_registers=(0, 2, 0x20))
        SIM.command_ready(peer)
        peer.write(bytes.fromhex("26010100"))
        with self.assertRaisesRegex(ValueError, "Unexpected helper destination"):
            peer.read(1)
        self.assertEqual(peer.calls, [])
        self.assertEqual(peer.snapshot(), peer.before)


if __name__ == "__main__":
    unittest.main()
