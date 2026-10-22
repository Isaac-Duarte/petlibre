"""Execute actual packet validation, flash dispatch and readback offline."""
import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("recovery_session", ROOT / "tools/emulate/emulate-recovery-session.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


@evidence(TOOL.HELPER.exists(), "Requires private pinned vendor helper")
class RecoverySessionTests(unittest.TestCase):
    def receiver(self):
        port = TOOL.EmulatedHelperPort(TOOL.HELPER.read_bytes())
        TOOL.command_ready(port)
        port.write(b"\x07")
        TOOL.READER.expect(port, 6)
        return port

    def test_actual_single_sector_erase_program_and_readback(self):
        report = TOOL.execute()
        self.assertEqual(report["program_packets"], 4)
        self.assertEqual(report["readback_packets"], 4)
        self.assertEqual(report["erase_calls"], 1)
        self.assertEqual(report["eight_byte_program_calls"], 512)
        self.assertTrue(report["outside_target_unchanged"])

    def test_checksum_failure_rejects_before_program(self):
        port = self.receiver()
        frame = bytearray(TOOL.program_frame(1, TOOL.TARGET, b"\x5a" * 1024))
        frame[-1] ^= 1
        port.write(bytes(frame))
        self.assertEqual(port.read(1), b"\x15")
        self.assertEqual(port.calls, [])
        self.assertEqual(port.snapshot(), port.before)

    def test_nak_after_bad_sequence_or_complement_does_not_mean_unchanged_flash(self):
        for fault in ("sequence", "complement"):
            with self.subTest(fault=fault):
                port = self.receiver()
                frame = bytearray(TOOL.program_frame(1, TOOL.TARGET, b"\x5a" * 1024))
                if fault == "sequence":
                    frame[1:3] = bytes([2, 253])
                else:
                    frame[2] ^= 1
                port.write(bytes(frame))
                self.assertEqual(port.read(1), b"\x15")
                self.assertEqual(sum(c["name"] == "program" for c in port.calls), 128)
                self.assertEqual(sum(c["name"] == "erase" for c in port.calls), 0)
                after = port.snapshot()
                self.assertNotEqual(after[TOOL.TARGET:TOOL.TARGET + 1024], port.before[TOOL.TARGET:TOOL.TARGET + 1024])
                self.assertEqual(after[:TOOL.TARGET], port.before[:TOOL.TARGET])
                self.assertEqual(after[TOOL.TARGET + 1024:], port.before[TOOL.TARGET + 1024:])

    def test_duplicate_packet_ack_does_not_program_twice(self):
        port = self.receiver()
        frame = TOOL.program_frame(1, TOOL.TARGET, b"\x5a" * 1024)
        port.write(frame)
        self.assertEqual(port.read(1), b"\x06")
        calls = list(port.calls)
        port.write(frame)
        self.assertEqual(port.read(1), b"\x06")
        self.assertEqual(port.calls, calls)

    def test_frame_builder_rejects_partial_or_outside_sector_input(self):
        for number, offset, data in ((0, TOOL.TARGET, b"x" * 1024),
                                      (1, TOOL.TARGET, b"x"),
                                      (1, TOOL.TARGET - 1024, b"x" * 1024)):
            with self.assertRaises(ValueError):
                TOOL.program_frame(number, offset, data)


if __name__ == "__main__":
    unittest.main()
