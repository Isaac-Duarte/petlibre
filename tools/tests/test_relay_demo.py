"""Relay demo patterns against a fake relay board; no serial port."""
import importlib.util
from pathlib import Path
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("relay_demo", ROOT / "tools/bench/relay-demo.py")
D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(D)


class FakeRelay:
    def __init__(self):
        self.calls = []

    def set(self, channel, on):
        self.calls.append((channel, on))


class DemoTests(unittest.TestCase):
    def test_patterns_switch_only_the_spares(self):
        relay = FakeRelay()
        show = D.Show(relay, 100)
        with mock.patch.object(D.time, "sleep"):
            show.chase()
            show.blink()
            show.heartbeat()
            show.morse("sos")
        self.assertTrue(relay.calls)
        self.assertEqual({c for c, _ in relay.calls}, {3, 4})
        self.assertEqual(show.state, {3: False, 4: False})
        self.assertGreaterEqual(show.step, D.MIN_STEP)

    def test_feeder_relays_and_the_toggle_cap_are_refused(self):
        show = D.Show(FakeRelay(), 1)
        with mock.patch.object(D.time, "sleep"):
            for channel in (1, 2):
                with self.assertRaises(D.B.BenchError):
                    show.set(channel, True)
            show.toggles = D.MAX_TOGGLES
            with self.assertRaises(D.B.BenchError):
                show.set(3, True)

    def test_refuses_while_a_bench_command_runs(self):
        with mock.patch.object(D, "busy", return_value="flash-execute"), \
                mock.patch.object(D.B, "open_relay") as opened, self.assertRaises(SystemExit):
            D.main([])
        opened.assert_not_called()


if __name__ == "__main__":
    unittest.main()
