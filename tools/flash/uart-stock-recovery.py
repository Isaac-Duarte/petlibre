#!/usr/bin/env python3
"""Prepare or execute exact original bank-1 recovery on the research feeder.

Continues an established pinned helper session; never uploads, changes status,
enters a loader or boots an application. An invalid target marker is required
for changed-bank recovery. No retries, resume, alternate images or other banks.
The explicit activation-test mode instead accepts only an intact original bank
and temporarily stages its marker sector for a separately reviewed bench test.
The separate bank-writeback-test mode intentionally erases and rewrites all
336 original-stock bank-1 sectors, withholding the marker until verified.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


COORDINATOR = module("serial_recovery_coordinator", "flash/recovery-transaction.py")
READER = module("serial_recovery_reader", "flash/uart-flash-read.py")
PLAN = COORDINATOR.PLAN
MARKER = module("serial_marker_test", "flash/marker-activation-test.py")
canonical, digest, require = COORDINATOR.canonical, COORDINATOR.digest, COORDINATOR.require
SOURCES = ("flash/uart-stock-recovery.py", "flash/recovery-transaction.py", "flash/recovery-sector-transport.py",
           "flash/plan-stock-recovery.py", "flash/uart-flash-read.py", "flash/flash-status.py", "flash/marker-activation-test.py")
RECOVERY_KIND = "original_stock_bank1_recovery"
MODES = {"prepare": RECOVERY_KIND, "execute": RECOVERY_KIND,
         "prepare-activation-test": MARKER.MARKER_KIND, "activation-test": MARKER.MARKER_KIND,
         "prepare-bank-writeback-test": MARKER.BANK_KIND, "bank-writeback-test": MARKER.BANK_KIND}


def operation_plan(baseline, current, kind):
    require(kind in MODES.values(), "Unknown bank-1 operation")
    if kind == RECOVERY_KIND:
        return PLAN.plan(baseline, current)
    return MARKER.plan(baseline, current, full_bank=kind == MARKER.BANK_KIND)


def private_file(path):
    path = Path(path)
    require(path.resolve().is_relative_to((ROOT / "private").resolve()) and path.is_file(),
            "Input must be a regular private file")
    return path.read_bytes()


def prepare_inputs(args):
    """All local provenance checks run before serial access or output creation."""
    baseline, current = private_file(args.baseline), private_file(args.current)
    require(args.mode in MODES, "Unknown preparation/execution mode")
    kind = MODES[args.mode]
    recovery_plan = operation_plan(baseline, current, kind)
    READER.load_helper(ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin")
    READER.validate_rom_report(args.rom_report)
    READER.validate_status_session(args.small_report, args.port)
    report_path = args.current.parent / "report.json"
    raw_report = private_file(report_path)
    report = json.loads(raw_report)
    require(report.get("mode") == "read-full" and report.get("status") == "complete" and
            report.get("port") == args.port and report.get("helper_sha256") == READER.HELPER_SHA256 and
            report.get("helper_upload_attempted") is False and
            report.get("bounded_read_report_sha256") == digest(private_file(args.small_report)) and
            report.get("result", {}).get("offset") == 0 and
            report.get("result", {}).get("bytes") == PLAN.CAPACITY and
            report.get("result", {}).get("sha256") == digest(current),
            "Complete full-read report from this helper session required")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(digest(private_file(report_path.parent / name)) == report.get(name + "_sha256"),
                "Full-read serial evidence hash mismatch")
    reviewed = {"schema_version": 1, "kind": kind,
                "port": args.port, "recovery_plan": recovery_plan,
                "current_report_sha256": digest(raw_report),
                "bounded_read_report_sha256": digest(private_file(args.small_report)),
                "rom_report_sha256": digest(private_file(args.rom_report)),
                "tool_sha256": {name: digest((ROOT / "tools" / name).read_bytes()) for name in SOURCES}}
    return baseline, current, reviewed


def comparable(reviewed):
    """Exclude only the nested planner's creation timestamp, not any guard."""
    result = json.loads(canonical(reviewed))
    if result.get("kind") == "original_stock_bank1_recovery":
        require(isinstance(result.get("recovery_plan", {}).get("created_utc"), str),
                "Plan creation time required")
        del result["recovery_plan"]["created_utc"]
    return canonical(result)


def validate_reviewed(raw, expected_sha256, baseline, current, expected):
    require(digest(raw) == expected_sha256, "Reviewed plan SHA256 differs")
    reviewed = json.loads(raw)
    require(comparable(reviewed) == comparable(expected), "Reviewed plan/source/evidence differs")
    if reviewed["kind"] == RECOVERY_KIND:
        COORDINATOR.validate_plan(baseline, current, reviewed["recovery_plan"])
    else:
        require(canonical(operation_plan(baseline, current, reviewed["kind"])) ==
                canonical(reviewed["recovery_plan"]), "Write-back acceptance plan differs")
    return reviewed


def execute(args, baseline, current, reviewed, raw_plan):
    """Open once; coordinator owns durable mutation intent and activation order."""
    require(reviewed["kind"] in MODES.values(), "Unknown bank-1 operation")
    directory = READER.output_directory(args.output)
    with (directory / "reviewed-plan.json").open("xb") as stream:
        stream.write(raw_plan)
        stream.flush()
        os.fsync(stream.fileno())
    COORDINATOR.sync_directory(directory)
    COORDINATOR.sync_directory(directory.parent)
    session = {"status": "prepared", "start_utc": COORDINATOR.utc(), "port": args.port,
               "operation": reviewed["kind"],
               "reviewed_plan_sha256": digest(raw_plan), "helper_upload_attempted": False,
               "snapshots": [], "transaction_directory": "transaction",
               "physical_recovery_verified": False,
               "scope": "Supervised serial stock restore; physical acceptance requires external bench evidence"}
    port, serial_port = None, None

    def save():
        (directory / "session.json").write_text(json.dumps(session, indent=2) + "\n")

    def snapshot():
        number = len(session["snapshots"]) + 1
        partial = directory / f"snapshot-{number:02d}.bin.part"
        with partial.open("xb") as stream:
            result = READER.receive_flash(port, 0, PLAN.CAPACITY, stream,
                                         lambda n, total: print(f"Snapshot {number}: {n}/{total}", flush=True))
            stream.flush()
            os.fsync(stream.fileno())
        data = partial.read_bytes()
        require(len(data) == PLAN.CAPACITY, "Full snapshot truncated")
        final = partial.with_suffix("")
        partial.rename(final)
        COORDINATOR.sync_directory(directory)
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
        port = READER.EvidencePort(serial_port, directory)
        session["status"] = "running"
        save()
        print("Original bank-1 operation; fresh full verification before any mutation", flush=True)
        if reviewed["kind"] == RECOVERY_KIND:
            report = COORDINATOR.run_transaction(port, baseline, current, reviewed["recovery_plan"],
                                                 snapshot, directory / "transaction")
        else:
            def progress(number, total):
                if number == 1 or number == total or number % 16 == 0:
                    print(f"Original stock sectors verified: {number}/{total}", flush=True)

            report = MARKER.run(port, baseline, current, reviewed["recovery_plan"],
                                snapshot, directory / "transaction",
                                full_bank=reviewed["kind"] == MARKER.BANK_KIND, progress=progress)
        session.update(status="verified", snapshot_sha256=report["snapshot_sha256"])
        print("Bank-1 recovery and preserved ranges verified; keep application boot excluded until review", flush=True)
        return 0
    except (Exception, KeyboardInterrupt) as error:
        session.update(status="stopped", error=f"{type(error).__name__}: {error}")
        print("Stopped without replay. Keep application boot excluded and review private evidence.", flush=True)
        return 1
    finally:
        if port:
            port.close()
        elif serial_port:
            serial_port.close()
        session["end_utc"] = COORDINATOR.utc()
        for name in ("serial-rx.bin", "serial-tx.bin"):
            p = directory / name
            if p.exists():
                session[name + "_sha256"] = digest(p.read_bytes())
        save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=tuple(MODES))
    for name in ("baseline", "current", "rom-report", "small-report", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--port", required=True)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--expected-plan-sha256")
    args = parser.parse_args()
    baseline, current, expected = prepare_inputs(args)
    os.umask(0o077)
    if args.mode.startswith("prepare"):
        require(args.output.resolve().is_relative_to((ROOT / "private").resolve()), "Private output required")
        raw = canonical(expected) + b"\n"
        with args.output.open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        COORDINATOR.sync_directory(args.output.parent)
        print("Prepared bank-1 recovery plan SHA256:", digest(raw))
        print("Operation:", expected["kind"])
        return 0
    require(args.plan is not None and args.expected_plan_sha256 is not None, "Exact reviewed plan/hash required")
    raw = private_file(args.plan)
    reviewed = validate_reviewed(raw, args.expected_plan_sha256, baseline, current, expected)
    require(MODES[args.mode] == reviewed["kind"],
            "Explicit execution mode differs from reviewed operation")
    return execute(args, baseline, current, reviewed, raw)


if __name__ == "__main__":
    raise SystemExit(main())
