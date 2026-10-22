#!/usr/bin/env python3
"""Inspect, prepare and execute a bank-1 give-way (FW09/FW11).

Clears bank 1's first marker byte with one program frame, so the factory
loader falls back to bank 0. Only when bank 0 holds exactly a frozen,
reviewed image. Continues an established helper session (bench.py loader and
read-small); never uploads a helper, boots or retries. Restoring bank 1
afterwards is a development update with `--restore-marker`.
"""
import argparse
import json
import os
from pathlib import Path

from importlib.util import module_from_spec, spec_from_file_location

ROOT = Path(__file__).resolve().parents[2]
SPEC = spec_from_file_location("give_way_cli", ROOT / "tools/flash/uart-development.py")
U = module_from_spec(SPEC)
SPEC.loader.exec_module(U)
B, C, T, R = U.B, U.C, U.T, U.R
require, digest, canonical = U.require, U.digest, U.canonical
SOURCES = ("flash/uart-give-way.py", *U.SOURCES)


def inputs(args):
    fallback, fallback_review = U.frozen(args.fallback, B.BANK0)
    baseline = T.pinned_baseline(U.private_file(args.baseline))
    R.load_helper(ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin")
    R.validate_rom_report(args.rom_report)
    R.validate_status_session(args.small_report, args.port)
    bindings = {"schema_version": 1, "kind": B.GIVE_WAY_KIND, "port": args.port,
                "fallback_review_sha256": fallback_review, "baseline_sha256": digest(baseline),
                "rom_report_sha256": digest(U.private_file(args.rom_report)),
                "bounded_read_report_sha256": digest(U.private_file(args.small_report)),
                "tool_sha256": {n: digest((ROOT / "tools" / n).read_bytes()) for n in SOURCES}}
    return baseline, fallback, bindings


def extents(fallback):
    return (("factory", 0, 0x6000), ("bank0", B.BANK0.start, len(B.payload(fallback, B.BANK0))),
            ("bank1-sector", B.START, B.SECTOR))


def inspected(path, bindings, fallback, baseline):
    raw = U.private_file(path)
    report = json.loads(raw)
    require(report.get("status") == "complete" and report.get("mode") == "inspect" and
            canonical(report.get("bindings")) == canonical(bindings) and
            report.get("helper_upload_attempted") is False, "Fresh matching inspection required")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(digest(U.private_file(path.parent / name)) == report.get(name + "_sha256"),
                "Inspection serial evidence mismatch")
    observations = report.get("observations", [])
    require(len(observations) == 1 and len(observations[0]) == 3, "Complete guard acquisition required")
    values = []
    for transfer, (_, offset, length) in zip(observations[0], extents(fallback)):
        value = U.private_file(path.parent / transfer["file"])
        require(transfer["offset"] == offset and transfer["bytes"] == length == len(value) and
                transfer["sha256"] == digest(value), "Inspection extent/hash differs")
        values.append(value)
    require(values[0] == baseline[:0x6000], "Factory region differs from the pinned baseline")
    B.give_way_plan(baseline, fallback, values[1], values[2])
    return values[1], values[2], digest(raw)


def session(args, baseline, fallback, bindings, bank0=None, bank1=None, reviewed=None, raw_plan=None):
    if args.mode == "execute":
        require(raw_plan is not None and digest(raw_plan) == args.expected_plan_sha256 and
                canonical(json.loads(raw_plan)) == canonical(reviewed), "Exact reviewed plan/hash required")
    directory = R.output_directory(args.output)
    if raw_plan is not None:
        with (directory / "reviewed-plan.json").open("xb") as stream:
            stream.write(raw_plan)
            stream.flush()
            os.fsync(stream.fileno())
    report = {"mode": args.mode, "status": "prepared", "start_utc": C.utc(), "bindings": bindings,
              "helper_upload_attempted": False, "observations": [], "status_checks": [],
              "application_boot_verified": False}

    def save():
        (directory / "report.json").write_text(json.dumps(report, indent=2) + "\n")

    port = serial_port = None
    save()
    try:
        import serial
        serial_port = serial.Serial(port=None, baudrate=115200, timeout=.05, write_timeout=3,
                                    xonxoff=False, rtscts=False, dsrdtr=False, exclusive=True)
        serial_port.dtr = serial_port.rts = False
        serial_port.port = args.port
        serial_port.open()
        port = R.EvidencePort(serial_port, directory)
        report["status"] = "running"
        save()

        def acquire():
            number = len(report["observations"]) + 1
            observation, values = [], []
            for label, offset, length in extents(fallback):
                partial = directory / f"{label}-{number:02d}.bin.part"
                with partial.open("xb") as stream:
                    result = R.receive_flash(port, offset, length, stream)
                    stream.flush()
                    os.fsync(stream.fileno())
                data = partial.read_bytes()
                require(len(data) == length, "Incomplete guard read")
                final = partial.with_suffix("")
                partial.rename(final)
                observation.append({"file": final.name, "offset": offset, **result, "sha256": digest(data)})
                values.append(data)
            report["observations"].append(observation)
            save()
            print(f"Guard acquisition {number} complete", flush=True)
            return tuple(values)

        if args.mode == "inspect":
            factory, region, sector = acquire()
            require(factory == baseline[:0x6000], "Factory region differs from the pinned baseline")
            B.give_way_plan(baseline, fallback, region, sector)
            T.check_status(port, B.START, B.START + 1024, "inspection",
                           lambda event, **fields: report["status_checks"].append({"event": event, **fields}))
            report["status"] = "complete"
        else:
            result = B.run_give_way(port, baseline, fallback, bank0, bank1, reviewed["operation_plan"],
                                    acquire, directory / "transaction")
            report.update(status="verified", result=result, reviewed_plan_sha256=digest(raw_plan))
        print("Completed; boot remains a separate bench check", flush=True)
        return 0
    except (Exception, KeyboardInterrupt) as error:
        report.update(status="stopped", error=f"{type(error).__name__}: {error}")
        print("Stopped without replay; inspect the evidence", flush=True)
        return 1
    finally:
        if port:
            port.close()
        elif serial_port:
            serial_port.close()
        report["end_utc"] = C.utc()
        for name in ("serial-rx.bin", "serial-tx.bin"):
            if (directory / name).exists():
                report[name + "_sha256"] = digest((directory / name).read_bytes())
        save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("inspect", "prepare", "execute"))
    for name in ("baseline", "fallback", "rom-report", "small-report", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--port", required=True)
    parser.add_argument("--inspection", type=Path)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--expected-plan-sha256")
    args = parser.parse_args()
    os.umask(0o077)
    baseline, fallback, bindings = inputs(args)
    if args.mode == "inspect":
        return session(args, baseline, fallback, bindings)
    require(args.inspection is not None, "Completed inspection report required")
    bank0, bank1, report_sha = inspected(args.inspection, bindings, fallback, baseline)
    reviewed = {**bindings, "inspection_report_sha256": report_sha,
                "operation_plan": B.give_way_plan(baseline, fallback, bank0, bank1)}
    if args.mode == "prepare":
        require(args.output.resolve().is_relative_to((ROOT / "private").resolve()), "Private plan required")
        raw = canonical(reviewed) + b"\n"
        with args.output.open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        print("Prepared give-way plan SHA256:", digest(raw))
        return 0
    require(args.plan is not None and args.expected_plan_sha256 is not None, "Reviewed plan and hash required")
    raw = U.private_file(args.plan)
    require(digest(raw) == args.expected_plan_sha256 and canonical(json.loads(raw)) == canonical(reviewed),
            "Reviewed plan/source/evidence differs")
    return session(args, baseline, fallback, bindings, bank0, bank1, reviewed, raw)


if __name__ == "__main__":
    raise SystemExit(main())
