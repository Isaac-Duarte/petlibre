"""The dashboard reads bench status files only; no ports, no writes."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bench_dashboard", Path(__file__).resolve().parents[1] / "bench/bench-dashboard.py")
DASH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DASH)


class DashboardTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.status = root / "private/bench-status"
        self.captures = root / "private/captures"
        self.status.mkdir(parents=True)
        (self.captures / "run-boot").mkdir(parents=True)
        for name, value in (("ROOT", root), ("STATUS_DIR", self.status), ("CAPTURES", self.captures)):
            patcher = patch.object(DASH, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, **state):
        base = {"pid": os.getpid(), "command": "flash-execute", "name": "run", "started": time.time() - 5,
                "finished": None, "outcome": "running", "error": None, "steps": [], "progress": None,
                "capture": None, "relay": {}, "lines": []}
        (self.status / "current.json").write_text(json.dumps({**base, **state}))

    def test_empty(self):
        self.assertEqual(DASH.snapshot()["current"], None)

    def test_live_capture_tail_is_printable_and_bounded(self):
        rx = b"".join(b"PLOPT1 window n=%d\r\n\x00\x1b" % n for n in range(100))
        (self.captures / "run-boot/serial-rx.bin").write_bytes(rx)
        self.write(capture={"path": "private/captures/run-boot", "seconds": 180, "started": time.time() - 10})
        current = DASH.snapshot()["current"]
        self.assertTrue(current["alive"])
        capture = current["capture"]
        self.assertEqual(capture["bytes"], len(rx))
        self.assertEqual(len(capture["tail"]), DASH.TAIL_LINES)
        self.assertEqual(capture["tail"][-1], "PLOPT1 window n=99")
        self.assertTrue(9 <= capture["elapsed"] <= 11)

    def test_latest_rx_log_is_the_newest_capture(self):
        self.assertIsNone(DASH.snapshot()["rx"])
        old = self.captures / "old"
        old.mkdir()
        (old / "serial-rx.bin").write_bytes(b"old\r\n")
        os.utime(old / "serial-rx.bin", (1, 1))
        (self.captures / "run-boot/serial-rx.bin").write_bytes(b"PLFW1 start\r\nPLOPT1 open\r\n")
        rx = DASH.snapshot()["rx"]
        self.assertEqual((rx["path"], rx["tail"]), ("private/captures/run-boot", ["PLFW1 start", "PLOPT1 open"]))

    def test_update_transfer_counts_session_bytes(self):
        session = self.captures.parent / "verification/run-execute"
        session.mkdir(parents=True)
        (session / "serial-tx.bin").write_bytes(b"x" * 300)
        (session / "serial-rx.bin").write_bytes(b"y" * 700)
        self.write(transfer={"path": "private/verification/run-execute", "tx": 1000, "rx": 3000})
        self.assertEqual(DASH.snapshot()["current"]["transfer"],
                         {"tx": 300, "rx": 700, "tx_expected": 1000, "rx_expected": 3000, "started": None})
        self.write(transfer={"path": "private/captures/run-boot", "tx": 1, "rx": 1})
        self.assertIsNone(DASH.snapshot()["current"]["transfer"])

    def test_operator_script_starts_at_the_anchor_arrival(self):
        self.assertIsNone(DASH.snapshot()["script"])
        script = {"title": "t", "capture": "private/captures/run-boot", "anchor": "PLOPT1 open",
                  "steps": [{"at": 0, "text": "Hands off"}, {"at": 20, "text": "Block"}]}
        (self.status / "script.json").write_text(json.dumps(script))
        self.assertIsNone(DASH.snapshot()["script"]["started"])
        d = self.captures / "run-boot"
        (d / "serial-rx.bin").write_bytes(b"PLLED1 done\r\nPLOPT1 open board=1\r\n")
        (d / "serial-events.jsonl").write_text(
            json.dumps({"utc": "2026-10-22T00:00:00+00:00", "direction": "rx", "offset": 0, "bytes": 13}) + "\n"
            + json.dumps({"utc": "2026-10-22T00:00:05+00:00", "direction": "rx", "offset": 13, "bytes": 20}) + "\n")
        view = DASH.snapshot()["script"]
        self.assertEqual(view["started"], 1792627205.0)
        self.assertEqual(view["steps"][1]["text"], "Block")
        script["capture"] = "private/bench-status"
        (self.status / "script.json").write_text(json.dumps(script))
        self.assertIsNone(DASH.snapshot()["script"])

    def test_capture_outside_private_captures_is_ignored(self):
        self.write(capture={"path": "private/bench-status", "seconds": 1, "started": time.time()})
        self.assertIsNone(DASH.snapshot()["current"]["capture"])

    def test_a_dead_running_process_reads_as_ended(self):
        self.write(pid=2 ** 22 + 12345)
        self.assertEqual(DASH.snapshot()["current"]["outcome"], "ended")

    def test_history_newest_first(self):
        lines = [json.dumps({"command": c, "name": "x", "started": 1, "finished": 2, "outcome": "pass", "error": None})
                 for c in ("dump", "boot")]
        (self.status / "history.jsonl").write_text("\n".join(lines) + "\n")
        self.assertEqual([h["command"] for h in DASH.snapshot()["history"]], ["boot", "dump"])


if __name__ == "__main__":
    unittest.main()
