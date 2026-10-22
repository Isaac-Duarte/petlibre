"""FW30 transient auditor on synthetic captures; no analyzer or device."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "driver_transients", Path(__file__).resolve().parents[1] / "audit/audit-driver-transients.py")
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
CH = {"ina": 0, "inb": 1, "outa": 2, "outb": 3}
OFF, BRAKE, FWD, REV = 0b0000, 0b0011, 0b0001, 0b0010
RATE = 1_000_000


def capture(*segments):
    """segments: (value, samples); bits 4-7 carry unrelated noise."""
    return b"".join(bytes([value | 0x50]) * n for value, n in segments)


class TransientTests(unittest.TestCase):
    def check(self, data, mode="boot"):
        return M.check_cycle(data, CH, mode, RATE)

    def test_clean_boot_and_loader(self):
        r = self.check(capture((OFF, 3_000_000), (BRAKE, 2_000_000)))
        self.assertEqual((r["outcome"], r["brake_at_s"]), ("pass", 3.0))
        self.assertEqual(self.check(capture((OFF, 5_000_000)), "loader")["outcome"], "pass")

    def test_a_single_sample_drive_is_caught_and_timed(self):
        r = self.check(capture((OFF, 1000), (FWD, 1), (OFF, 10), (BRAKE, 100)))
        self.assertEqual(r["outcome"], "transients_input_drive")
        self.assertEqual(r["input_drive"], [{"start_us": 1000.0, "length_us": 1.0, "direction": "forward"}])
        r = self.check(capture((OFF, 10), (REV, 5), (BRAKE, 10)))
        self.assertEqual(r["input_drive"][0]["direction"], "reverse")
        # Inputs equal but an output pulse (OUTA high) is still a drive.
        r = self.check(capture((OFF, 10), (0b0100, 2), (BRAKE, 10)))
        self.assertEqual((r["outcome"], r["output_drive"][0]["high"]), ("transients_output_drive", "outa"))

    def test_state_rules(self):
        cases = [
            (capture((BRAKE, 10)), "boot", "transients_late_start"),
            (capture((OFF, 100)), "boot", "transients_no_brake"),
            (capture((OFF, 10), (BRAKE, 10), (OFF, 10)), "boot", "transients_no_brake"),
            (capture((OFF, 10), (BRAKE, 10), (OFF, 3), (BRAKE, 10)), "boot", "transients_left_brake"),
            (capture((OFF, 10), (BRAKE, 10)), "loader", "transients_loader_driven"),
            (b"", "boot", "transients_empty"),
        ]
        for data, mode, outcome in cases:
            with self.subTest(outcome=outcome):
                self.assertEqual(self.check(data, mode)["outcome"], outcome)

    def test_session_summary_and_integrity(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            files = {"cycle-001.bin": capture((OFF, 2000), (BRAKE, 1000)),
                     "cycle-002.bin": capture((OFF, 2500), (BRAKE, 500))}
            for name, data in files.items():
                (d / name).write_bytes(data)
            manifest = {"schema_version": 1, "mode": "boot", "samplerate": RATE, "seconds": 8, "channels": CH,
                        "cycles": [{"file": n, "bytes": len(b)} for n, b in files.items()]}
            (d / "manifest.json").write_text(json.dumps(manifest))
            report = M.verify(d)
            self.assertEqual((report["outcome"], report["passed"]), ("pass", 2))
            self.assertEqual(report["brake_at_s"], {"min": 0.002, "max": 0.0025})
            (d / "cycle-002.bin").write_bytes(files["cycle-002.bin"] + b"\0")
            with self.assertRaisesRegex(ValueError, "changed since capture"):
                M.verify(d)
            manifest["channels"] = {"ina": 0, "inb": 0, "outa": 2, "outb": 3}
            (d / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "four distinct channels"):
                M.verify(d)


if __name__ == "__main__":
    unittest.main()
