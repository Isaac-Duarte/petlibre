#!/usr/bin/env python3
"""Show off the relay bench: click the spare relays in a few patterns.

Only relays 3 and 4 (unused spares) are switched. Feeder power (1) and the
loader strap (2) are read, never set, so the feeder is untouched. Refuses
while another bench command is running; the relay port is opened
exclusively as well. Toggles are rate-limited and capped to spare the
contacts, and both spares end off whatever happens. Progress shows on the
bench dashboard like any other command.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import time

from importlib.util import module_from_spec, spec_from_file_location

ROOT = Path(__file__).resolve().parents[2]
SPEC = spec_from_file_location("relay_demo_bench", ROOT / "tools/bench/bench.py")
B = module_from_spec(SPEC)
SPEC.loader.exec_module(B)

SPARES = (3, 4)
MIN_STEP = 0.12      # Seconds between toggles; the board clicks audibly.
MAX_TOGGLES = 120    # Per run, to bound contact wear.
MORSE = {"P": ".--.", "E": ".", "T": "-", "L": ".-..", "I": "..", "B": "-...", "R": ".-.", "O": "---",
         "K": "-.-", "S": "...", " ": " "}


def busy():
    """A live bench command, from the dashboard status file."""
    try:
        state = json.loads((B.STATUS_DIR / "current.json").read_text())
    except (OSError, ValueError):
        return None
    if state.get("outcome") != "running":
        return None
    try:
        os.kill(int(state.get("pid", 0)), 0)
    except (OSError, ValueError):
        return None
    return state.get("command")


class Show:
    def __init__(self, relay, speed):
        self.relay, self.step, self.toggles = relay, max(MIN_STEP, 0.25 / speed), 0
        self.state = dict.fromkeys(SPARES, False)

    def set(self, channel, on, hold=1.0):
        require_spare(channel)
        if self.state[channel] != on:
            if self.toggles >= MAX_TOGGLES:
                raise B.BenchError(f"Toggle cap ({MAX_TOGGLES}) reached")
            self.relay.set(channel, on)
            self.state[channel] = on
            self.toggles += 1
        time.sleep(self.step * hold)

    def both(self, a, b, hold=1.0):
        self.set(3, a, 0)
        self.set(4, b, hold)

    def chase(self, rounds=4):
        for _ in range(rounds):
            self.both(True, False)
            self.both(False, True)
        self.both(False, False)

    def blink(self, times=3):
        for _ in range(times):
            self.both(True, True, 2)
            self.both(False, False, 2)

    def heartbeat(self, beats=3):
        for _ in range(beats):
            self.both(True, True, .6)
            self.both(False, False, .6)
            self.both(True, True, .6)
            self.both(False, False, 4)

    def morse(self, text):
        for letter in text.upper():
            for symbol in MORSE.get(letter, ""):
                if symbol == " ":
                    time.sleep(self.step * 4)
                    continue
                self.set(3, True, 1 if symbol == "." else 3)
                self.set(3, False, 1)
            time.sleep(self.step * 2)


def require_spare(channel):
    B.require(channel in SPARES, f"Relay {channel} is not a spare; the demo never switches it")


PATTERNS = ("chase", "blink", "heartbeat", "morse")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("patterns", nargs="*", help=f"Any of {', '.join(PATTERNS)}; default all, in order")
    parser.add_argument("--speed", type=float, default=1.0, help="Faster above 1 (rate-limited)")
    parser.add_argument("--text", default="OK", help="Morse message on relay 3")
    args = parser.parse_args(argv)
    unknown = sorted(set(args.patterns) - set(PATTERNS))
    if unknown:
        parser.error(f"unknown pattern {', '.join(unknown)}; choose from {', '.join(PATTERNS)}")
    running = busy()
    if running:
        parser.exit(1, f"error: bench command '{running}' is running; try again when it ends\n")
    config = B.load_config()
    B.STATUS = B.Status(B.STATUS_DIR)
    B.STATUS.begin("relay-demo", None, list(args.patterns or PATTERNS))
    relay = B.open_relay(config)
    show = Show(relay, args.speed)
    code, error = 0, None
    try:
        power, strap = relay.get(B.POWER), relay.get(B.STRAP)
        print(f"Feeder power {'on' if power else 'off'}, strap {'held' if strap else 'released'}: left as is")
        for name in args.patterns or PATTERNS:
            B.STATUS.step(name)
            print(f"{name}...", flush=True)
            show.morse(args.text) if name == "morse" else getattr(show, name)()
        print(f"Done: {show.toggles} toggles")
    except (Exception, KeyboardInterrupt) as failure:
        code, error = 1, f"{type(failure).__name__}: {failure}"
        print("Stopped:", error, file=sys.stderr)
    finally:
        for channel in SPARES:
            try:
                relay.set(channel, False)
            except Exception as failure:  # Report, then keep switching the other off.
                code, error = 1, f"Spare {channel} may still be on: {failure}"
                print(error, file=sys.stderr)
        relay.port.close()
        B.STATUS.finish(code, error)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
