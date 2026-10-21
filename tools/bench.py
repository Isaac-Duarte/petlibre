#!/usr/bin/env python3
"""Drive the relay bench: feeder power, loader strap, captures and supervised updates.

Relay 1 switches the feeder's USB 5V and relay 2 holds feeder TX to GND through
the 4.7 kΩ strap. Serial work is delegated unchanged to uart-flash-read.py and
uart-development.py, so their pinned checks, plan hashes and no-retry rules
apply. This tool never writes to the feeder UART itself, never retries, and
never boots after a stopped update. Ports and evidence paths come from the
ignored private/bench.json; see docs/hardware/bench.md.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "private/bench.json"
POWER, STRAP = 1, 2
STRAP_SECONDS = 2.0
LOADER_SECONDS = 5


class BenchError(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise BenchError(message)


def load_config(path=CONFIG):
    config = json.loads(Path(path).read_text())
    for key in ("relay_port", "uart_port", "baseline", "rom_report"):
        require(isinstance(config.get(key), str), f"{path.name} needs {key}")
    return config


class Relay:
    """DSD TECH SH-UR04A: CP2102, 9600 8N1, AT+CHn=0/1 and AT+CHn=? queries."""

    def __init__(self, port):
        self.port = port

    def command(self, text):
        self.port.reset_input_buffer()
        self.port.write(text.encode() + b"\r\n")
        reply = self.port.read_until(b"\n").strip(b"\0\r\n")
        require(reply.startswith(b"OK"), f"Relay refused {text}: {reply!r}")
        return reply

    def set(self, channel, on):
        value = int(bool(on))
        reply = self.command(f"AT+CH{channel}={value}")
        require(reply == f"OK+CH{channel}={value}".encode(), f"Relay {channel} reply {reply!r}")

    def get(self, channel):
        reply = self.command(f"AT+CH{channel}=?")
        require(reply in (f"OK+CH{channel}=0".encode(), f"OK+CH{channel}=1".encode()),
                f"Relay {channel} state {reply!r}")
        return reply.endswith(b"1")


def open_relay(config):
    import serial
    return Relay(serial.Serial(config["relay_port"], 9600, timeout=1, exclusive=True))


def tool(name, *args):
    return [sys.executable, str(ROOT / "tools" / name), *map(str, args)]


def run(command):
    print("+", Path(command[1]).name, " ".join(command[2:]), flush=True)
    return subprocess.run(command, cwd=ROOT).returncode


def power_off(relay, seconds):
    relay.set(STRAP, False)
    relay.set(POWER, False)
    time.sleep(seconds)


def start_observer(config, output, seconds, timeout=10):
    """Start a receive-only capture and wait until it holds the UART port."""
    process = subprocess.Popen(tool("uart-flash-read.py", "observe", "--port", config["uart_port"],
                                    "--output", output, "--seconds", seconds), cwd=ROOT)
    report = Path(output) / "report.json"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        require(process.poll() is None, "Observer exited before holding the port")
        try:
            if json.loads(report.read_text()).get("status") == "running":
                return process
        except (OSError, ValueError):
            pass
        time.sleep(0.05)
    process.kill()
    raise BenchError("Observer did not hold the port in time")


def finish_observer(process, output):
    require(process.wait() == 0, f"Observer failed; see {output}")
    return json.loads((Path(output) / "report.json").read_text())["result"]


def boot(relay, config, output, seconds, off_seconds):
    """Cold boot into the installed application while recording it from the start."""
    power_off(relay, off_seconds)
    observer = start_observer(config, output, seconds)
    try:
        relay.set(POWER, True)
    except BaseException:
        observer.kill()
        raise
    result = finish_observer(observer, output)
    require(not result["download_banner_seen"], "Boot reached the ROM loader, not the application")
    print(f"Boot captured: {result['bytes']} bytes in {output}; fault marker "
          f"{'seen' if result['fault_marker_seen'] else 'not seen'}", flush=True)
    return result


def loader(relay, config, output, off_seconds):
    """Cold boot with the strap held; leave the feeder powered in the ROM loader."""
    power_off(relay, off_seconds)
    observer = start_observer(config, output, LOADER_SECONDS)
    try:
        relay.set(STRAP, True)
        relay.set(POWER, True)
        time.sleep(STRAP_SECONDS)
        relay.set(STRAP, False)
        result = finish_observer(observer, output)
        require(result["download_banner_seen"] and result["longest_nak_run"] >= 8 and
                not result["fault_marker_seen"] and not result["stock_startup_markers"],
                f"Loader not confirmed; see {output}")
    except BaseException:
        observer.kill()
        power_off(relay, 0)
        print("Feeder powered off", file=sys.stderr)
        raise
    print(f"Loader confirmed: banner and {result['nak_bytes']} NAKs in {output}", flush=True)
    return result


def paths(name):
    return {"loader": ROOT / f"private/captures/{name}-loader",
            "bounded": ROOT / f"private/flash-backups/{name}-bounded",
            "inspect": ROOT / f"private/verification/{name}-inspect",
            "plan": ROOT / f"private/verification/{name}-plan.json",
            "execute": ROOT / f"private/verification/{name}-execute",
            "boot": ROOT / f"private/captures/{name}-boot"}


def update_args(config, args, p):
    common = ["--baseline", ROOT / config["baseline"], "--source", args.source, "--target", args.target,
              "--rom-report", ROOT / config["rom_report"], "--small-report", p["bounded"] / "report.json",
              "--port", config["uart_port"]]
    return common + (["--interrupted"] if args.interrupted else [])


def flash_prepare(relay, config, args):
    """Loader, helper bounded read, inspection and plan; stop for plan review."""
    p = paths(args.name)
    for key in ("loader", "bounded", "inspect", "plan"):
        require(not p[key].exists(), f"Refusing to reuse {p[key].relative_to(ROOT)}")
    loader(relay, config, p["loader"], args.off_seconds)
    steps = [
        tool("uart-flash-read.py", "read-small", "--port", config["uart_port"], "--output", p["bounded"],
             "--rom-report", ROOT / config["rom_report"]),
        tool("uart-development.py", "inspect", *update_args(config, args, p), "--output", p["inspect"]),
        tool("uart-development.py", "prepare", *update_args(config, args, p),
             "--inspection", p["inspect"] / "report.json", "--output", p["plan"]),
    ]
    for step in steps:
        if run(step):
            print("Stopped without retry; the feeder stays powered. Inspect the evidence.", flush=True)
            return 1
    raw = p["plan"].read_bytes()
    plan = json.loads(raw)["operation_plan"]
    print(f"Plan {p['plan'].relative_to(ROOT)}: {len(plan['ordered_sectors'])} sectors staged, "
          f"{plan['erase_sectors']} erases, target {plan['target_ota_sha256']}", flush=True)
    print(f"Review it, then: tools/bench.py flash-execute {args.name} --source {args.source} "
          f"--target {args.target} --expected-plan-sha256 {hashlib.sha256(raw).hexdigest()}", flush=True)
    return 0


def dump(relay, config, args):
    """Loader, helper bounded read, then a full 8 MiB read in the same helper session."""
    bounded = ROOT / f"private/flash-backups/{args.name}-bounded"
    full = ROOT / f"private/flash-backups/{args.name}-full"
    for path in (bounded, full):
        require(not path.exists(), f"Refusing to reuse {path.relative_to(ROOT)}")
    loader(relay, config, ROOT / f"private/captures/{args.name}-loader", args.off_seconds)
    read = ("--port", config["uart_port"], "--rom-report", ROOT / config["rom_report"])
    for step in (tool("uart-flash-read.py", "read-small", *read, "--output", bounded),
                 tool("uart-flash-read.py", "read-full", *read, "--output", full,
                      "--small-report", bounded / "report.json")):
        if run(step):
            print("Stopped without retry; the feeder stays powered. Inspect the evidence.", flush=True)
            return 1
    print(f"Full read in {full.relative_to(ROOT)}; compare it before relying on it", flush=True)
    return 0


def flash_execute(relay, config, args):
    """Execute a reviewed plan in the still-powered helper session, then boot."""
    p = paths(args.name)
    require(p["plan"].is_file() and not p["execute"].exists(), "Prepared plan required; execution is single-use")
    # Relay state shows the supply was not switched off; it cannot rule out a brownout.
    require(relay.get(POWER) and not relay.get(STRAP), "Feeder power was cycled since prepare; start over")
    code = run(tool("uart-development.py", "execute", *update_args(config, args, p),
                    "--inspection", p["inspect"] / "report.json", "--plan", p["plan"],
                    "--expected-plan-sha256", args.expected_plan_sha256, "--output", p["execute"]))
    if code:
        print("Update stopped; the feeder stays powered and is not booted. No retry.", flush=True)
        return 1
    boot(relay, config, p["boot"], args.boot_seconds, args.off_seconds)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--off-seconds", type=float, default=2.0, help="Power-off time before a cold boot")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("status", help="Show relay states and ports")
    sub.add_parser("off", help="Release the strap and cut feeder power")
    sub.add_parser("on", help="Power the feeder without the strap or a capture")
    command = sub.add_parser("capture", help="Record the running feeder without touching power")
    command.add_argument("name", help="Capture name under private/captures/")
    command.add_argument("--seconds", type=float, default=60)
    for name, text in (("boot", "Cold boot into the application while capturing"),
                       ("loader", "Cold boot into the ROM loader and leave it powered")):
        command = sub.add_parser(name, help=text)
        command.add_argument("name", help="Capture name under private/captures/")
        if name == "boot":
            command.add_argument("--seconds", type=float, default=30)
    command = sub.add_parser("dump", help="Loader, bounded read, then full flash read")
    command.add_argument("name", help="Session name under private/flash-backups/")
    for name, text in (("flash-prepare", "Loader, bounded read, inspect and plan"),
                       ("flash-execute", "Execute the reviewed plan, then boot")):
        command = sub.add_parser(name, help=text)
        command.add_argument("name", help="Session name for private evidence paths")
        command.add_argument("--source", type=Path, required=True, help="Frozen review of the installed image")
        command.add_argument("--target", type=Path, required=True, help="Frozen review to install")
        command.add_argument("--interrupted", action="store_true")
        if name == "flash-execute":
            command.add_argument("--expected-plan-sha256", required=True)
            command.add_argument("--boot-seconds", type=float, default=30)
    args = parser.parse_args(argv)
    os.umask(0o077)
    config = load_config()
    relay = open_relay(config)
    try:
        if args.command == "status":
            for channel in (1, 2, 3, 4):
                print(f"Relay {channel}: {'on' if relay.get(channel) else 'off'}")
            for key in ("relay_port", "uart_port"):
                print(f"{key}: {'present' if Path(config[key]).exists() else 'missing'}")
        elif args.command == "off":
            power_off(relay, 0)
        elif args.command == "on":
            relay.set(STRAP, False)
            relay.set(POWER, True)
        elif args.command == "capture":
            output = ROOT / "private/captures" / args.name
            result = finish_observer(start_observer(config, output, args.seconds), output)
            print(f"Captured {result['bytes']} bytes in {output}", flush=True)
        elif args.command == "boot":
            boot(relay, config, ROOT / "private/captures" / args.name, args.seconds, args.off_seconds)
        elif args.command == "loader":
            loader(relay, config, ROOT / "private/captures" / args.name, args.off_seconds)
        elif args.command == "dump":
            return dump(relay, config, args)
        elif args.command == "flash-prepare":
            return flash_prepare(relay, config, args)
        else:
            return flash_execute(relay, config, args)
        return 0
    except BenchError as error:
        print(f"Stopped: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
