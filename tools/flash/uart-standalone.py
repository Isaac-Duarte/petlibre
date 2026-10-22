#!/usr/bin/env python3
"""Prepare/execute the pinned UART diagnostic installation or original restore.

Continue a fresh established helper session. Never upload, enter a loader, change
baud/status registers, reset, boot, retry or resume. Separate install/restore
modes and an externally reviewed plan hash are mandatory before serial access.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("standalone_bank", ROOT / "tools/flash/standalone-bank.py")
BANK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BANK)
C, R = BANK.C, BANK.R
require, digest, canonical = BANK.require, BANK.digest, BANK.canonical
receive_snapshot = R.receive_flash
SOURCES = ("flash/uart-standalone.py", "flash/standalone-bank.py", "flash/recovery-transaction.py",
           "flash/recovery-sector-transport.py", "flash/plan-stock-recovery.py", "flash/uart-flash-read.py", "flash/flash-status.py")
MODES = {"prepare-install": "install", "install": "install", "prepare-restore": "restore", "restore": "restore"}


def private_file(path):
    path = Path(path)
    require(path.resolve().is_relative_to((ROOT / "private").resolve()) and path.is_file(), "Regular private input required")
    return path.read_bytes()


def prepare_inputs(args):
    require(args.mode in MODES, "Explicit standalone mode required")
    baseline, current, candidate = (private_file(getattr(args, n)) for n in ("baseline", "current", "candidate"))
    operation = BANK.plan(baseline, current, candidate, MODES[args.mode])
    R.load_helper(ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin")
    R.validate_rom_report(args.rom_report)
    R.validate_status_session(args.small_report, args.port)
    full_report = private_file(args.current.parent / "report.json")
    report = json.loads(full_report)
    require(report.get("mode") == "read-full" and report.get("status") == "complete" and
            report.get("port") == args.port and report.get("helper_sha256") == R.HELPER_SHA256 and
            report.get("helper_upload_attempted") is False and
            report.get("bounded_read_report_sha256") == digest(private_file(args.small_report)) and
            report.get("result", {}).get("offset") == 0 and
            report.get("result", {}).get("bytes") == 0x800000 and
            report.get("result", {}).get("sha256") == digest(current),
            "Complete current full read from this uninterrupted helper session required")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(digest(private_file(args.current.parent / name)) == report.get(name + "_sha256"),
                "Full-read serial hash mismatch")
    reviewed = {"schema_version": 1, "kind": operation["kind"], "port": args.port,
                "operation_plan": operation, "current_report_sha256": digest(full_report),
                "bounded_read_report_sha256": digest(private_file(args.small_report)),
                "rom_report_sha256": digest(private_file(args.rom_report)),
                "tool_sha256": {name: digest((ROOT / "tools" / name).read_bytes()) for name in SOURCES}}
    return baseline, current, candidate, reviewed


def validate_reviewed(raw, expected_sha256, expected):
    require(digest(raw) == expected_sha256, "Reviewed plan SHA256 differs")
    reviewed = json.loads(raw)
    require(canonical(reviewed) == canonical(expected), "Reviewed standalone plan/source/evidence differs")
    return reviewed


def execute(args, baseline, current, candidate, reviewed, raw_plan):
    require(args.mode in ("install", "restore") and reviewed["kind"] == BANK.KINDS[args.mode],
            "Execution direction differs from reviewed operation")
    require(canonical(reviewed["operation_plan"]) == canonical(BANK.plan(baseline, current, candidate, args.mode)),
            "Operation plan differs before opening serial")
    directory = R.output_directory(args.output)
    with (directory / "reviewed-plan.json").open("xb") as stream:
        stream.write(raw_plan)
        stream.flush()
        os.fsync(stream.fileno())
    C.sync_directory(directory)
    C.sync_directory(directory.parent)
    session = {"status": "prepared", "start_utc": C.utc(), "port": args.port,
               "operation": reviewed["kind"], "reviewed_plan_sha256": digest(raw_plan),
               "helper_upload_attempted": False, "snapshots": [], "transaction_directory": "transaction",
               "physical_boot_verified": False,
               "scope": "Supervised serial operation; physical identity/boot acceptance require bench evidence"}
    port, serial_port = None, None

    def save():
        (directory / "session.json").write_text(json.dumps(session, indent=2) + "\n")

    def snapshot():
        number = len(session["snapshots"]) + 1
        partial = directory / f"snapshot-{number:02d}.bin.part"
        with partial.open("xb") as stream:
            result = receive_snapshot(port, 0, 0x800000, stream,
                                     lambda n, total: print(f"Snapshot {number}: {n}/{total}", flush=True))
            stream.flush()
            os.fsync(stream.fileno())
        data = partial.read_bytes()
        require(len(data) == 0x800000, "Full snapshot truncated")
        final = partial.with_suffix("")
        partial.rename(final)
        C.sync_directory(directory)
        session["snapshots"].append({"file": final.name, "sha256": digest(data), **result})
        save()
        return data

    save()
    try:
        import serial
        serial_port = serial.Serial(port=None, baudrate=115200, timeout=.05, write_timeout=3,
                                    xonxoff=False, rtscts=False, dsrdtr=False, exclusive=True)
        serial_port.dtr = serial_port.rts = False
        serial_port.port = args.port
        serial_port.open()
        port = R.EvidencePort(serial_port, directory)
        session["status"] = "running"
        save()
        print(f"Pinned diagnostic {args.mode}; full preimage verification before any mutation", flush=True)

        def progress(number, total):
            if number == 1 or number % 8 == 0 or number == total:
                print(f"Verified sectors: {number}/{total}", flush=True)

        report = BANK.run(port, baseline, current, candidate, args.mode, reviewed["operation_plan"],
                          snapshot, directory / "transaction", progress)
        session.update(status="verified", snapshot_sha256=report["snapshot_sha256"])
        print("Image and all preserved bytes verified; keep application boot excluded until bench review", flush=True)
        return 0
    except (Exception, KeyboardInterrupt) as error:
        session.update(status="stopped", error=f"{type(error).__name__}: {error}")
        print("Stopped without replay. Keep application boot excluded; inspect private evidence.", flush=True)
        return 1
    finally:
        if port:
            port.close()
        elif serial_port:
            serial_port.close()
        session["end_utc"] = C.utc()
        for name in ("serial-rx.bin", "serial-tx.bin"):
            if (directory / name).exists():
                session[name + "_sha256"] = digest((directory / name).read_bytes())
        save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=tuple(MODES))
    for name in ("baseline", "current", "candidate", "rom-report", "small-report", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--port", required=True)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--expected-plan-sha256")
    args = parser.parse_args()
    baseline, current, candidate, expected = prepare_inputs(args)
    os.umask(0o077)
    if args.mode.startswith("prepare-"):
        require(args.output.resolve().is_relative_to((ROOT / "private").resolve()), "Private output required")
        raw = canonical(expected) + b"\n"
        with args.output.open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        C.sync_directory(args.output.parent)
        print("Prepared", expected["kind"], "plan SHA256:", digest(raw))
        return 0
    require(args.plan is not None and args.expected_plan_sha256 is not None, "Exact reviewed plan and SHA256 required")
    raw = private_file(args.plan)
    reviewed = validate_reviewed(raw, args.expected_plan_sha256, expected)
    require(reviewed["kind"] == BANK.KINDS[args.mode], "Explicit execution mode differs")
    return execute(args, baseline, current, candidate, reviewed, raw)


if __name__ == "__main__":
    raise SystemExit(main())
