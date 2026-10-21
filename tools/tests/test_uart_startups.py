"""Offline evidence-integrity and reset-decoder checks; no device I/O."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest
from support import evidence as requires_evidence

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("uart_startups", ROOT / "tools/audit-uart-startups.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


def evidence(rx, tx=b""):
    events = []
    for direction, data in (("rx", rx), ("tx", tx)):
        if data:
            events.append({"direction": direction, "offset": 0, "bytes": len(data),
                           "utc": "2026-10-19T23:00:00+00:00"})
    metadata = {"status": "complete", "rx_sha256": TOOL.digest(rx),
                "tx_sha256": TOOL.digest(tx)}
    return events, metadata


class UartStartupTests(unittest.TestCase):
    def test_split_markers_and_correlated_rejection(self):
        first = b"Flash ID:c8,40,17\napp_main:version:1.1.15\nRecv AssocRsp with status_"
        second = b"code=30\n"
        third = b"Flash ID:c8,40,17\napp_main:version:1.1.15\n"
        rx = first + second + third
        events, metadata = evidence(rx)
        events = [{"direction": "rx", "offset": offset, "bytes": len(data),
                   "utc": f"2026-10-19T23:00:{seconds:02d}+00:00"}
                  for offset, data, seconds in [(0, first, 0), (len(first), second, 10),
                                                (len(first + second), third, 16)]]
        report = TOOL.analyze(rx, b"", events, metadata)
        self.assertEqual(len(report["application_starts"]), 2)
        self.assertEqual(report["association_responses"][0]["status"], 30)
        self.assertEqual(report["startup_intervals"][0]["rejection_to_flash_seconds"], 16)
        self.assertEqual(report["startup_intervals"][0]["seconds"], 16)

    def test_only_fixed_numeric_evidence_is_emitted(self):
        secret = b"DO_NOT_EMIT_DEVICE_PASSWORD"
        rx = (secret + b'\n[ota_persist] recover enter reg3=00000000 reg4=00000000 '
              b'boot=00100000 nvs_ret=1 msg_id=private\n')
        events, metadata = evidence(rx)
        report = TOOL.analyze(rx, b"", events, metadata)
        self.assertEqual(report["reported_boot_reasons"][0]["value"], "00100000")
        self.assertNotIn(secret.decode(), json.dumps(report))
        self.assertNotIn("msg_id", json.dumps(report))

    def test_finalized_hashes_and_complete_status_are_required(self):
        events, metadata = evidence(b"data")
        for change in ({"rx_sha256": "0" * 64}, {"status": "running"}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                TOOL.analyze(b"data", b"", events, metadata | change)
        del metadata["rx_sha256"]
        with self.assertRaisesRegex(ValueError, "hash missing"):
            TOOL.analyze(b"data", b"", events, metadata)

    def test_event_loss_reordering_and_truncation_are_rejected(self):
        events, metadata = evidence(b"abc")
        for change in ({"offset": 1}, {"bytes": 2}, {"bytes": 4},
                       {"bytes": True}, {"direction": "invalid"},
                       {"utc": "2026-10-19T23:00:00"}):
            altered = copy.deepcopy(events)
            altered[0].update(change)
            with self.subTest(change=change), self.assertRaises(ValueError):
                TOOL.analyze(b"abc", b"", altered, metadata)
        with self.assertRaisesRegex(ValueError, "coverage"):
            TOOL.analyze(b"abc", b"", [], metadata)

    def test_timestamps_must_be_monotonic_across_directions(self):
        events, metadata = evidence(b"abc", b"?")
        events[1]["utc"] = "2026-10-19T22:59:59+00:00"
        with self.assertRaisesRegex(ValueError, "backward"):
            TOOL.analyze(b"abc", b"?", events, metadata)

    def test_standard_recorder_metadata_and_tx_accounting(self):
        events, metadata = evidence(b"app_main:version:1.1.15\n", b"?")
        metadata["serial-rx.bin_sha256"] = metadata.pop("rx_sha256")
        metadata["serial-tx.bin_sha256"] = metadata.pop("tx_sha256")
        self.assertEqual(TOOL.analyze(b"app_main:version:1.1.15\n", b"?", events,
                                     metadata)["tx_bytes"], 1)
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            TOOL.analyze(b"app_main:version:1.1.15\n", b"!", events, metadata)

    def test_empty_capture_is_not_a_boot_or_fault(self):
        events, metadata = evidence(b"")
        report = TOOL.analyze(b"", b"", events, metadata)
        self.assertEqual(report["application_starts"], [])
        self.assertFalse(report["recognized_fault_marker_seen"])

    @requires_evidence(TOOL.FLASH.exists() and TOOL.ROM.exists(), "Private reset fixtures unavailable")
    def test_actual_factory_distinguishes_watchdog_and_software_reset_flags(self):
        report = TOOL.reset_cases(TOOL.FLASH.read_bytes(), TOOL.ROM.read_bytes())
        cases = {r["case"]: r["rom_reason"] for r in report["cases"]}
        self.assertEqual(cases["km4_watchdog"], "00100000")
        self.assertEqual(cases["km4_system_and_watchdog"], "00080000")
        self.assertEqual(cases["cold"], "00000000")


if __name__ == "__main__":
    unittest.main()
