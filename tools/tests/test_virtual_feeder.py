"""End-to-end replacement Rust behavior on synthetic hardware and persistent NOR."""

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("virtual_feeder", ROOT / "tools/emulate/virtual-feeder.py")
VF = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VF)


def action(at, op, *args, **values):
    return {"at_ms": at, "op": op, **({"values": values} if op == "expect" else {"args": list(args)})}


def scenario(actions, end=80, profile="single", feedback=10):
    return {"version": 1, "name": "integration", "profile": profile,
            "feedback_period_ms": feedback, "end_ms": end, "actions": actions}


class VirtualFeederTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        subprocess.run(["make", "-C", "firmware/sim"], cwd=ROOT, check=True, capture_output=True)

    def run_case(self, actions, **kwargs):
        report = VF.run_scenario(scenario(actions, **kwargs))
        self.assertTrue(report["passed"], [a for a in report["assertions"] if not a["passed"]])
        return report

    def test_single_and_double_bowl_complete_through_real_journal(self):
        for profile in ("single", "double"):
            with self.subTest(profile=profile):
                report = self.run_case([
                    action(1, "submit", 1, 3),
                    action(1, "expect", submission="enqueued"),
                    action(90, "expect", state="terminal", completed=3, outcome="done",
                           durable=True, motor=0, optics=0, pending=False, high_water_id=1),
                ], end=95, profile=profile)
                drives = {e["motor"] for e in report["events"] if e["event"] == "outputs"}
                self.assertIn(1, drives)
                if profile == "double":
                    self.assertIn(2, drives)

    def test_mailbox_ids_acknowledgement_and_next_request(self):
        self.run_case([
            action(0, "submit", 0, 1), action(0, "expect", submission="reject_id"),
            action(0, "submit", 1, 0), action(0, "expect", submission="reject_request"),
            action(1, "submit", 1, 1),
            action(2, "submit", 2, 1), action(2, "expect", submission="reject_busy"),
            action(8, "cancel", 99), action(8, "expect", response=0),
            action(25, "ack", 99), action(25, "expect", response=0),
            action(26, "expect", state="terminal", durable=True, completed=1),
            action(27, "ack", 1), action(28, "expect", state="idle"),
            action(28, "submit", 1, 1), action(28, "expect", submission="reject_id"),
            action(29, "submit", 2, 1), action(29, "expect", submission="enqueued"),
            action(70, "expect", state="terminal", id=2, completed=1, durable=True),
        ])

    def test_interruption_invalidates_double_bowl_phase(self):
        self.run_case([
            action(1, "submit", 1, 3), action(8, "cancel", 1),
            action(20, "expect", outcome="cancelled", durable=True, phase_known=False, motor=0),
            action(21, "ack", 1), action(22, "submit", 2, 1),
            action(22, "expect", submission="reject_phase"),
        ], profile="double", end=25)

    def test_faults_stop_and_finish_without_success(self):
        cases = [
            ("missing", [action(8, "auto", 0)], {"reason": "feedback_timeout"}),
            ("outlet", [action(8, "inputs", 4)], {"reason": "outlet"}),
            ("current", [action(8, "current", 4095)], {"reason": "overcurrent"}),
            ("power", [action(8, "inputs", 1), action(8, "battery", 0)], {"reason": "power"}),
            ("adc", [action(8, "adc_stuck", 1)], {"state": "fault", "fault": "hardware"}),
            ("timer", [action(8, "timer", 0)], {"state": "fault", "fault": "hardware"}),
        ]
        for name, events, expected in cases:
            with self.subTest(fault=name):
                report = self.run_case([action(1, "submit", 1, 3), *events,
                    action(50, "expect", durable=True, motor=0, optics=0, completed=0, **expected)], end=55)
                self.assertFalse(any(e.get("event") == "status" and e.get("outcome") == 1
                                     for e in report["events"]))

    def test_stalled_owner_independent_timer_and_watchdog_reset(self):
        report = self.run_case([
            action(1, "submit", 1, 3), action(8, "owner", 0),
            action(25, "expect", motor=0, optics=0),
            action(150, "expect", boots=2, state="fault", fault="recovery", pending=True, motor=0),
        ], end=155)
        self.assertEqual(sum(e["event"] == "watchdog_reset_requested" for e in report["events"]), 1)

    def test_reboot_after_completion_rejects_replay_and_accepts_new_id(self):
        self.run_case([
            action(1, "submit", 10, 1), action(25, "expect", durable=True, completed=1),
            action(26, "power_cut"), action(26, "expect", online=False),
            action(27, "boot"), action(27, "expect", state="idle", high_water_id=10, pending=False),
            action(28, "submit", 10, 1), action(28, "expect", submission="reject_id"),
            action(29, "submit", 11, 1), action(29, "expect", submission="enqueued"),
            action(60, "expect", durable=True, completed=1, id=11),
        ])

    def test_power_cut_before_reservation_does_not_consume_id(self):
        self.run_case([
            action(1, "submit", 1, 1), action(1, "power_cut"),
            action(2, "boot"), action(2, "expect", state="idle", high_water_id=0),
            action(3, "submit", 1, 1), action(3, "expect", submission="enqueued"),
            action(30, "expect", completed=1, durable=True),
        ], end=35)

    def test_power_cut_during_motion_blocks_recovery_and_new_feed(self):
        report = self.run_case([
            action(1, "submit", 1, 3), action(8, "expect", state="feeding", motor=1),
            action(8, "power_cut"), action(9, "boot"),
            action(9, "expect", state="fault", fault="recovery", pending=True, high_water_id=1),
            action(10, "submit", 2, 1), action(10, "expect", submission="reject_stopped"),
            action(30, "expect", motor=0, optics=0, durable=False),
        ], end=35)
        self.assertFalse(any(e["boot"] == 2 and e["event"] == "outputs" and e["motor"] in (1, 2)
                             for e in report["events"]))

    def test_byte_cuts_at_erase_body_and_commit_boundaries(self):
        for phase in (1, 2):
            for byte in (0, 1, 256, 257, 270, 380, 381, 383, 384):
                with self.subTest(phase=phase, byte=byte):
                    # A committed reservation is pending; a committed completion is idle.
                    if phase == 1 and byte <= 256:
                        expected = {"state": "idle", "high_water_id": 0}
                    elif phase == 2 and byte == 384:
                        expected = {"state": "idle", "high_water_id": 1, "pending": False}
                    else:
                        expected = {"state": "fault", "fault": "recovery"}
                    report = self.run_case([
                        action(0, "cut", phase, byte), action(1, "submit", 1, 1),
                        action(25, "expect", online=False), action(26, "boot"),
                        action(26, "expect", motor=0, optics=0, **expected),
                    ], end=30)
                    self.assertEqual(sum(e["event"] == "power_cut" for e in report["events"]), 1)
                    self.assertFalse(any(e["boot"] == 2 and e["event"] == "outputs" and e["motor"] in (1, 2)
                                         for e in report["events"]))

    def test_explicit_feedback_replaces_automatic_plant(self):
        report = self.run_case([
            action(1, "submit", 1, 1), action(15, "edge", 1),
            action(25, "expect", state="terminal", completed=1, durable=True),
        ], feedback=0, end=30)
        self.assertFalse(any(e["event"] == "feedback" for e in report["events"]))

    def test_identical_scenarios_have_identical_reports(self):
        data = scenario([action(1, "submit", 1, 1), action(30, "expect", completed=1)], end=35)
        self.assertEqual(VF.run_scenario(data), VF.run_scenario(data))

    def test_invalid_scenarios_rejected_before_worker_launch(self):
        base = scenario([action(1, "expect", online=True)])
        bad = []
        for key, value in (("version", 2), ("end_ms", 10001), ("profile", "stock"),
                           ("feedback_period_ms", True), ("actions", [])):
            item = copy.deepcopy(base); item[key] = value; bad.append(item)
        for event in (action(1, "shell", 0), action(-1, "status"), action(1, "current", 4096),
                      action(1, "submit", -1, 1), action(1, "expect", durable=1),
                      action(1, "expect", unknown=1), action(1, "expect", state="made_up")):
            item = copy.deepcopy(base); item["actions"] = [event]; bad.append(item)
        bad.append(scenario([action(2, "status"), action(1, "expect", online=True)]))
        for item in bad:
            with self.subTest(item=item), self.assertRaises(VF.HarnessError):
                VF.validate(item)

    def test_invalid_worker_commands_and_unexpected_exit(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "private/verification") as scratch:
            worker = VF.Worker(VF.WORKER, Path(scratch) / "journal.bin", 0, True)
            try:
                worker.read()
                with self.assertRaisesRegex(VF.HarnessError, "Worker exited 2"):
                    worker.command("current", [5000])
            finally:
                worker.close()
        with self.assertRaises(VF.HarnessError):
            self.run_case([action(1, "power_cut"), action(2, "submit", 1, 1),
                           action(3, "expect", online=False)])

    def test_cli_exit_codes_report_creation_and_no_overwrite(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "private/verification") as scratch:
            data = Path(scratch) / "scenario.json"
            report = Path(scratch) / "report.json"
            command = ["python3", str(ROOT / "tools/emulate/virtual-feeder.py"), "--scenario", str(data), "--report", str(report)]
            data.write_text(json.dumps(scenario([action(1, "expect", online=True)], end=2)))
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 2)
            data.write_text(json.dumps(scenario([action(1, "expect", completed=1)], end=2)))
            command[-1] = str(Path(scratch) / "failure.json")
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 1)
            data.write_text('{"version":2}')
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 2)

    def test_checked_in_examples(self):
        paths = sorted((ROOT / "tools/emulate/scenarios/virtual-feeder").glob("*.json"))
        self.assertGreaterEqual(len(paths), 8)
        for path in paths:
            with self.subTest(scenario=path.name):
                report = VF.run_scenario(json.loads(path.read_text()))
                self.assertTrue(report["passed"], [a for a in report["assertions"] if not a["passed"]])


if __name__ == "__main__":
    unittest.main()
