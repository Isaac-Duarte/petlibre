#!/usr/bin/env python3
"""Deterministic replacement Rust feeder scenarios; no device or network access."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
WORKER = ROOT / "private/firmware-tests/virtual-feeder-worker"
STATES = ["starting", "idle", "queued", "preparing", "feeding", "reporting", "terminal", "fault", "stopped"]
FAULTS = ["ok", "setup", "recovery", "storage", "hardware", "watchdog", "flash"]
OUTCOMES = ["pending", "done", "cancelled", "expired", "failed"]
REASONS = ["none", "done", "cancelled", "clock", "observation_gap", "total_timeout", "feedback_timeout", "power", "sensor", "outlet", "overcurrent", "feedback_stream", "output"]
SUBMISSIONS = ["enqueued", "reject_busy", "reject_id", "reject_request", "reject_phase", "reject_stopped"]
# Each command has fixed unsigned integer arguments and inclusive bounds.
COMMANDS = {
    "submit": [(0, 2**64 - 1), (0, 65535)],
    "cancel": [(0, 2**64 - 1)], "ack": [(0, 2**64 - 1)],
    "auto": [(0, 100000)], "edge": [(0, 1)],
    "owner": [(0, 1)], "timer": [(0, 1)], "adc_stuck": [(0, 1)],
    "current": [(0, 4095)], "battery": [(0, 4095)], "inputs": [(0, 7)],
    "cut": [(1, 2), (0, 100000)], "status": [], "stop": [],
    "power_cut": [], "boot": [], "expect": None,
}
STATUS_FIELDS = {"device_ms", "response", "state", "fault", "supervisor_fault", "watchdog_fault", "id", "completed", "outcome", "reason", "durable", "stopped", "optics_off", "phase_known", "high_water_id", "pending", "journal_error", "motor", "optics", "watchdog_resets", "online", "boots", "submission"}
BOOLEAN_FIELDS = {"durable", "stopped", "optics_off", "phase_known", "pending", "online"}
ENUM_FIELDS = {"state": STATES, "fault": FAULTS, "outcome": OUTCOMES, "reason": REASONS, "submission": SUBMISSIONS}


class HarnessError(RuntimeError):
    """Invalid scenario, worker failure, or malformed protocol."""


def integer(value, low, high):
    return type(value) is int and low <= value <= high


def validate(scenario):
    if not isinstance(scenario, dict) or set(scenario) - {"version", "name", "profile", "feedback_period_ms", "end_ms", "actions"}:
        raise HarnessError("Unknown scenario fields")
    if type(scenario.get("version")) is not int or scenario["version"] != 1:
        raise HarnessError("Scenario version must be 1")
    if not isinstance(scenario.get("name"), str) or not 1 <= len(scenario["name"]) <= 100:
        raise HarnessError("Scenario needs a name of 1..100 characters")
    if scenario.get("profile", "single") not in ("single", "double"):
        raise HarnessError("Profile must be single or double")
    if not integer(scenario.get("feedback_period_ms", 10), 0, 100000):
        raise HarnessError("Invalid feedback period")
    if not integer(scenario.get("end_ms"), 0, 10000):
        raise HarnessError("end_ms must be 0..10000")
    actions = scenario.get("actions")
    if not isinstance(actions, list) or len(actions) > 10000:
        raise HarnessError("actions must be a list with at most 10000 entries")
    previous = 0
    expectations = 0
    for action in actions:
        if not isinstance(action, dict) or set(action) - {"at_ms", "op", "args", "values"}:
            raise HarnessError("Invalid action fields")
        if not integer(action.get("at_ms"), previous, scenario["end_ms"]):
            raise HarnessError("Actions must have nondecreasing timestamps within end_ms")
        previous = action["at_ms"]
        op = action.get("op")
        if not isinstance(op, str) or op not in COMMANDS:
            raise HarnessError("Unknown action")
        if op == "expect":
            values = action.get("values")
            if "args" in action or not isinstance(values, dict) or not values or set(values) - STATUS_FIELDS:
                raise HarnessError("expect requires known, nonempty status values")
            for key, value in values.items():
                if key in BOOLEAN_FIELDS:
                    valid = type(value) is bool
                elif key in ENUM_FIELDS:
                    valid = isinstance(value, str) and value in ENUM_FIELDS[key]
                else:
                    valid = integer(value, 0, 2**64 - 1)
                if not valid:
                    raise HarnessError(f"Invalid expectation for {key}")
            expectations += 1
        else:
            args = action.get("args", [])
            bounds = COMMANDS[op]
            if "values" in action or not isinstance(args, list) or len(args) != len(bounds):
                raise HarnessError(f"Invalid arguments for {op}")
            if any(not integer(value, *bound) for value, bound in zip(args, bounds)):
                raise HarnessError(f"Arguments out of range for {op}")
    if not expectations:
        raise HarnessError("Scenario needs at least one expectation")
    return scenario


class Worker:
    """Bounded request/response transport with stderr retained for diagnostics."""

    def __init__(self, executable, image, mode, provision):
        self.errors = tempfile.TemporaryFile()
        self.process = subprocess.Popen(
            [str(executable), str(image), str(mode), str(int(provision))],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.errors,
        )
        self.buffer = b""
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)

    def read(self):
        deadline = time.monotonic() + 5
        events = []
        while True:
            while b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                try:
                    event = json.loads(line)
                except (ValueError, UnicodeError) as error:
                    raise HarnessError("Malformed worker JSON") from error
                if not isinstance(event, dict) or event.get("event") not in {"status", "outputs", "storage_begin", "storage_end", "feedback", "power_cut", "journal_io"}:
                    raise HarnessError("Unknown worker event")
                events.append(event)
                if event["event"] in {"status", "power_cut"}:
                    return events
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise HarnessError("Worker response timed out")
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data:
                self.process.wait(timeout=5)
                self.errors.seek(0)
                detail = self.errors.read(4096).decode(errors="replace")
                raise HarnessError(f"Worker exited {self.process.returncode}: {detail}")
            self.buffer += data
            if len(self.buffer) > 65536 or len(events) > 10000:
                raise HarnessError("Worker response exceeded bounds")

    def command(self, op, args=()):
        try:
            self.process.stdin.write((" ".join([op, *map(str, args)]) + "\n").encode())
            self.process.stdin.flush()
        except (BrokenPipeError, OSError) as error:
            raise HarnessError("Worker command pipe failed") from error
        return self.read()

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=5)
        self.selector.close()
        self.process.stdin.close()
        self.process.stdout.close()
        self.errors.close()


def identities(executable):
    rust = ROOT / "firmware/rust"
    paths = sorted(set(
        [p for crate in ("core", "amebad", "testkit", "sim") for p in (rust / crate).rglob("*")
         if p.suffix in (".rs", ".toml", ".h")]
        + list((ROOT / "firmware/sim").glob("*"))
        + [ROOT / "Cargo.lock", Path(__file__)]
    ))
    return {"worker_sha256": hashlib.sha256(Path(executable).read_bytes()).hexdigest(),
            "sources": {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                        for p in paths if p.is_file()}}


class Simulation:
    def __init__(self, scenario, executable, directory):
        self.scenario = validate(scenario)
        self.executable = executable
        self.image = Path(directory) / "journal.bin"
        self.worker = None
        self.now = 0
        self.boots = 0
        self.latest = {}
        self.last_submission = None
        self.events = []
        self.assertions = []
        self.armed = False
        self.reserved = False
        self.phase = 0

    def record(self, event):
        self.events.append({"at_ms": self.now, "boot": self.boots, **event})

    def check(self, label, ok, actual=None, expected=None):
        self.assertions.append({"at_ms": self.now, "label": label, "passed": bool(ok),
                                "actual": actual, "expected": expected})

    def consume(self, events):
        reset = False
        for event in events:
            self.record(event)
            kind = event["event"]
            if kind == "storage_begin":
                self.phase = event["phase"]
                if self.phase == 1:
                    self.reserved = False
            elif kind == "storage_end":
                if self.phase == 1 and event["ok"]:
                    self.reserved = True
                elif self.phase == 2:
                    self.reserved = False
                self.phase = 0
            elif kind == "outputs":
                if event["motor"] in (1, 2):
                    self.check("reservation_before_drive", self.reserved)
                self.check("valid_motor_pair", event["motor"] != 3)
            elif kind == "status":
                self.latest = dict(event)
                if event["durable"]:
                    self.check("durable_result_after_inactive_outputs",
                               event["motor"] == 0 and event["optics"] == 0
                               and event["stopped"] and event["optics_off"] and not event["pending"])
                reset = event["watchdog_resets"] > 0
            elif kind == "power_cut":
                if not self.armed:
                    raise HarnessError("Unexpected worker power cut")
                try:
                    code = self.worker.process.wait(timeout=5)
                except subprocess.TimeoutExpired as error:
                    raise HarnessError("Worker failed to exit after power cut") from error
                if code != 75:
                    raise HarnessError("Invalid power-cut worker exit")
                self.record({"event": "power_removed", "outputs": "unpowered"})
                self.offline()
        if reset and self.worker is not None:
            self.record({"event": "watchdog_reset_requested"})
            self.offline()
            self.boot()

    def offline(self):
        if self.worker:
            self.worker.close()
        self.worker = None
        self.armed = False
        self.reserved = False
        self.phase = 0

    def boot(self):
        if self.worker:
            raise HarnessError("boot requires the simulated power to be off")
        self.boots += 1
        self.last_submission = None
        self.worker = Worker(self.executable, self.image,
                             int(self.scenario.get("profile", "single") == "double"), self.boots == 1)
        self.record({"event": "boot", "profile": self.scenario.get("profile", "single")})
        self.consume(self.worker.read())
        self.send("auto", [self.scenario.get("feedback_period_ms", 10)])

    def send(self, op, args=()):
        if not self.worker:
            raise HarnessError(f"{op} requires simulated power")
        if op == "cut" or op == "power_cut":
            self.armed = True
        self.consume(self.worker.command(op, args))
        if op == "submit":
            response = self.latest.get("response")
            if not integer(response, 0, len(SUBMISSIONS) - 1):
                raise HarnessError("Invalid submission response")
            self.last_submission = SUBMISSIONS[response]

    def expect(self, values):
        actual = {**self.latest, "online": self.worker is not None, "boots": self.boots,
                  "submission": self.last_submission}
        for key, names in ENUM_FIELDS.items():
            if key in actual and isinstance(actual[key], int):
                if not integer(actual[key], 0, len(names) - 1):
                    raise HarnessError(f"Invalid worker enum {key}")
                actual[key] = names[actual[key]]
        for key, value in values.items():
            if not self.worker and key not in {"online", "boots"}:
                raise HarnessError("Only online/boots can be asserted while power is off")
            self.check(f"expect.{key}", actual.get(key) == value, actual.get(key), value)

    def run(self):
        try:
            self.boot()
            actions = iter(self.scenario["actions"])
            action = next(actions, None)
            for self.now in range(self.scenario["end_ms"] + 1):
                if self.now and self.worker:
                    self.send("tick")
                while action is not None and action["at_ms"] == self.now:
                    self.record({"event": "action", "action": action})
                    op = action["op"]
                    if op == "expect":
                        self.expect(action["values"])
                    elif op == "boot":
                        self.boot()
                    else:
                        self.send(op, action.get("args", []))
                    action = next(actions, None)
                if self.worker:
                    self.send("poll")
            return {"version": 1, "name": self.scenario["name"],
                    "profile": self.scenario.get("profile", "single"),
                    "scenario_sha256": hashlib.sha256(json.dumps(self.scenario, sort_keys=True).encode()).hexdigest(),
                    "identity": identities(self.executable),
                    "enums": {"state": STATES, "fault": FAULTS, "outcome": OUTCOMES,
                              "reason": REASONS, "submission": SUBMISSIONS,
                              "storage_phase": ["none", "reserve", "finish", "recover"],
                              "motor": ["stop", "drive_0", "drive_1", "invalid"]},
                    "limits": {"synthetic_hardware_and_timing": True, "physical_reliability_verified": False,
                               "full_boot_or_rtos_emulated": False, "real_flash_provider": False,
                               "network_or_scheduler_implemented": False},
                    "events": self.events, "assertions": self.assertions,
                    "passed": all(a["passed"] for a in self.assertions)}
        finally:
            self.offline()


def run_scenario(scenario, executable=WORKER):
    validate(scenario)
    directory = ROOT / "private/verification"
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="virtual-feeder-", dir=directory) as scratch:
        return Simulation(scenario, executable, scratch).run()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.scenario.stat().st_size > 2_000_000:
            raise HarnessError("Scenario exceeds 2 MB")
        scenario = validate(json.loads(args.scenario.read_text()))
        report = run_scenario(scenario)
        # Exclusive creation prevents accidental evidence replacement.
        with args.report.open("x") as output:
            json.dump(report, output, sort_keys=True, indent=2)
            output.write("\n")
        print(f"{'PASS' if report['passed'] else 'FAIL'}: {report['name']}")
        return 0 if report["passed"] else 1
    except (OSError, ValueError, HarnessError, subprocess.SubprocessError) as error:
        print(f"virtual-feeder: {error}", file=__import__("sys").stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
