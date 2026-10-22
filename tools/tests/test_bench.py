"""No device I/O: relay protocol, loader sequencing and update stop rules."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bench", Path(__file__).resolve().parents[1] / "bench/bench.py")
BENCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCH)
CONFIG = {"relay_port": "relay", "uart_port": "uart", "baseline": "b", "rom_report": "r"}


class FakeRelayPort:
    """Answers like the SH-UR04A, including its trailing NUL byte."""

    def __init__(self, refuse=False):
        self.state = {1: False, 2: False, 3: False, 4: False}
        self.log = []
        self.refuse = refuse
        self.pending = b""

    def reset_input_buffer(self):
        self.pending = b""

    def write(self, data):
        command = data.decode().strip()
        channel = int(command[5])
        if self.refuse or command[7:] not in ("0", "1", "?"):
            self.pending = b"Error\n\x00"
            return
        if command[7:] != "?":
            self.state[channel] = command[7:] == "1"
            self.log.append((channel, self.state[channel]))
        self.pending = f"OK+CH{channel}={int(self.state[channel])}\n\x00".encode()

    def read_until(self, terminator):
        reply, self.pending = self.pending, b""
        return reply


def loader_result(**changes):
    return {"download_banner_seen": True, "longest_nak_run": 99, "nak_bytes": 99,
            "fault_marker_seen": False, "stock_startup_markers": 0, "bytes": 120, **changes}


class RelayTests(unittest.TestCase):
    def test_set_and_query_match_device_replies(self):
        port = FakeRelayPort()
        relay = BENCH.Relay(port)
        relay.set(1, True)
        self.assertTrue(relay.get(1))
        self.assertFalse(relay.get(2))
        self.assertEqual(port.log, [(1, True)])

    def test_refusal_stops(self):
        with self.assertRaises(BENCH.BenchError):
            BENCH.Relay(FakeRelayPort(refuse=True)).set(1, True)


class SequenceTests(unittest.TestCase):
    def setUp(self):
        self.port = FakeRelayPort()
        self.relay = BENCH.Relay(self.port)
        sleep = patch.object(BENCH.time, "sleep")
        sleep.start()
        self.addCleanup(sleep.stop)

    def observer(self, result):
        process = SimpleNamespace(kill=lambda: None)
        return (patch.object(BENCH, "start_observer", return_value=process),
                patch.object(BENCH, "finish_observer", return_value=result))

    def test_loader_holds_strap_across_power_on_and_leaves_feeder_powered(self):
        start, finish = self.observer(loader_result())
        with start as started, finish:
            BENCH.loader(self.relay, CONFIG, Path("out"), 2)
        started.assert_called_once()
        self.assertEqual(self.port.log, [(2, False), (1, False), (2, True), (1, True), (2, False)])
        self.assertEqual(self.port.state, {1: True, 2: False, 3: False, 4: False})

    def test_unconfirmed_loader_powers_off(self):
        for result in (loader_result(download_banner_seen=False), loader_result(longest_nak_run=3),
                       loader_result(fault_marker_seen=True), loader_result(stock_startup_markers=1)):
            self.port.state.update({1: False, 2: False})
            start, finish = self.observer(result)
            with start, finish, self.assertRaises(BENCH.BenchError):
                BENCH.loader(self.relay, CONFIG, Path("out"), 2)
            self.assertEqual(self.port.state, {1: False, 2: False, 3: False, 4: False})

    def test_boot_refuses_loader_output(self):
        start, finish = self.observer(loader_result())
        with start, finish, self.assertRaises(BENCH.BenchError):
            BENCH.boot(self.relay, CONFIG, Path("out"), 10, 2)

    def test_execute_requires_uninterrupted_power(self):
        args = SimpleNamespace(name="unit-test-session", source=Path("s"), target=Path("t"),
                               interrupted=False, expected_plan_sha256="0" * 64,
                               boot_seconds=10, off_seconds=2)
        with patch.object(Path, "is_file", return_value=True), \
                patch.object(Path, "exists", return_value=False), \
                patch.object(BENCH, "run") as run, self.assertRaises(BENCH.BenchError):
            BENCH.flash_execute(self.relay, CONFIG, args)
        run.assert_not_called()

    def test_stopped_execute_never_boots_or_cuts_power(self):
        self.relay.set(1, True)
        args = SimpleNamespace(name="unit-test-session", source=Path("s"), target=Path("t"),
                               interrupted=False, expected_plan_sha256="0" * 64,
                               boot_seconds=10, off_seconds=2)
        with patch.object(Path, "is_file", return_value=True), \
                patch.object(Path, "exists", return_value=False), \
                patch.object(BENCH, "run", return_value=1) as run, \
                patch.object(BENCH, "boot") as boot:
            self.assertEqual(BENCH.flash_execute(self.relay, CONFIG, args), 1)
        run.assert_called_once()
        boot.assert_not_called()
        self.assertTrue(self.port.state[1])


class StatusTests(unittest.TestCase):
    def test_records_steps_progress_relays_and_history(self):
        import json, tempfile
        with tempfile.TemporaryDirectory() as tmp:
            status = BENCH.Status(Path(tmp))
            status.begin("dump", "unit", ["loader entry", "uart-flash-read read-full"])
            status.relay(1, True)
            status.step("loader entry")
            status.step("uart-flash-read read-full")
            status.line("Read 4096/8388608 bytes\n")
            status.write(force=True)
            current = json.loads((Path(tmp) / "current.json").read_text())
            self.assertEqual([s["state"] for s in current["steps"]], ["done", "running"])
            progress = current["progress"]
            self.assertEqual((progress["done"], progress["total"], progress["unit"]), (4096, 8388608, "bytes"))
            status.line("Read 8192/8388608 bytes\n")
            later = status.state["progress"]
            # The rate baseline stays at the first report of this count.
            self.assertEqual((later["done"], later["since"], later["since_done"]),
                             (8192, progress["since"], 4096))
            self.assertEqual(current["relay"], {"1": True})
            status.finish(1, "Observer failed")
            current = json.loads((Path(tmp) / "current.json").read_text())
            self.assertEqual((current["outcome"], current["steps"][1]["state"]), ("stopped", "failed"))
            history = [json.loads(line) for line in (Path(tmp) / "history.jsonl").read_text().splitlines()]
            self.assertEqual([(h["command"], h["outcome"], h["error"]) for h in history],
                             [("dump", "stopped", "Observer failed")])

    def test_disabled_status_writes_nothing(self):
        status = BENCH.Status()
        status.begin("boot", "unit", [])
        status.line("Read 1/2 bytes")
        status.finish(0)
        self.assertEqual(status.state["outcome"], "pass")


if __name__ == "__main__":
    unittest.main()
