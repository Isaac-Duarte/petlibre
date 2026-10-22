#!/usr/bin/env python3
"""Inspect, prepare and execute reviewed bank-1 application-region updates.

Continue an already established helper session. Never upload a helper, enter a
loader, change baud/status, boot an application or replay a stopped transaction.
`--interrupted` plans afresh from a new inspection of a stopped update's region
(see development-bank.py); it never reads or replays the stopped journal.
"""
import argparse
import json
import os
from pathlib import Path

from importlib.util import module_from_spec, spec_from_file_location

ROOT = Path(__file__).resolve().parents[2]
SPEC = spec_from_file_location("uart_development_bank", ROOT / "tools/flash/development-bank.py")
B = module_from_spec(SPEC)
SPEC.loader.exec_module(B)
C, T, R = B.C, B.T, B.R
require, digest, canonical = B.require, B.digest, B.canonical
SOURCES = ("flash/uart-development.py", "flash/development-bank.py", "flash/review-development-image.py",
           "flash/recovery-transaction.py", "flash/recovery-sector-transport.py", "flash/uart-flash-read.py",
           "flash/flash-status.py", "flash/plan-stock-recovery.py", "audit/audit-ota-layout.py")


def private_file(path):
    path = Path(path)
    require(path.resolve().is_relative_to((ROOT / "private").resolve()) and path.is_file(),
            "Regular private input required")
    return path.read_bytes()


def frozen(directory):
    raw = private_file(directory / "review.json")
    review = json.loads(raw)
    require(type(review.get("schema_version")) is int and review["schema_version"] == 1 and
            review.get("kind") == "plaf103_development_image_review", "Unsupported frozen review")
    image = private_file(directory / "image.bin")
    data = B.payload(image)
    require(review.get("ota_sha256") == digest(image) and
            review.get("payload_sha256") == digest(data) and
            type(review.get("payload_bytes")) is int and review["payload_bytes"] == len(data),
            "Frozen image/review mismatch")
    if review.get("image_kind") == "stock":
        require(digest(image) == B.L.STOCK_SHA256 and review.get("manifest_sha256") is None,
                "Only original stock may omit a build manifest")
    else:
        require(review.get("image_kind") == "build", "Unknown reviewed image kind")
        manifest_raw = private_file(directory / "build-manifest.json")
        require(digest(manifest_raw) == review.get("manifest_sha256"), "Frozen manifest mismatch")
        m = json.loads(manifest_raw)
        lock = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
        require(m.get("profile") == "plaf103-firstboot" and
                m.get("sdk_revision") == lock["sdk"]["revision"] and
                m.get("toolchain_sha256") == lock["toolchain"]["sha256"], "Unknown build profile")
        ota = [a for a in m.get("artifacts", []) if Path(a["path"]).name == "OTA_All.bin"]
        require(len(ota) == 1 and ota[0]["sha256"] == digest(image) and
                type(ota[0]["bytes"]) is int and ota[0]["bytes"] == len(image), "Manifest/image mismatch")
        require(review.get("review", {}).get("startup_mains") ==
                m.get("firstboot_startup_audit", {}).get("mains") and
                set(review["review"]["startup_mains"]) == {"lp", "hp"} and
                review["review"].get("pinmap") == m.get("km0_pinmap_audit") and
                m.get("km0_pinmap_audit", {}).get("matches_stock") is True,
                "Frozen linked/startup/pad review missing")
        require(review["review"].get("layout", {}).get("sha256") == digest(image) and
                [x.get("bank") for x in review["review"].get("factory_mapping", [])] == [0, 1] and
                all(m.get("checks", {}).get(k) is True for k in
                    ("ota_payload_matches", "ota_checksum_matches", "source_snapshot_unchanged",
                     "staged_sources_without_diagnostics")), "Incomplete build/mapping review")
    return image, digest(raw)


def inputs(args):
    source, source_review = frozen(args.source)
    target, target_review = frozen(args.target)
    baseline = T.pinned_baseline(private_file(args.baseline))
    R.load_helper(ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin")
    R.validate_rom_report(args.rom_report)
    R.validate_status_session(args.small_report, args.port)
    bindings = {"schema_version": 1, "kind": B.KIND, "port": args.port, "interrupted": args.interrupted,
                "source_review_sha256": source_review, "target_review_sha256": target_review,
                "baseline_sha256": digest(baseline),
                "rom_report_sha256": digest(private_file(args.rom_report)),
                "bounded_read_report_sha256": digest(private_file(args.small_report)),
                "tool_sha256": {n: digest((ROOT / "tools" / n).read_bytes()) for n in SOURCES}}
    return baseline, source, target, bindings


def inspected(path, bindings, source, target, baseline):
    raw = private_file(path)
    report = json.loads(raw)
    require(report.get("status") == "complete" and report.get("mode") == "inspect" and
            canonical(report.get("bindings")) == canonical(bindings) and
            report.get("helper_upload_attempted") is False, "Fresh matching inspection required")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(digest(private_file(path.parent / name)) == report.get(name + "_sha256"),
                "Inspection serial evidence mismatch")
    observations = report.get("observations", [])
    require(len(observations) == 1 and len(observations[0]) == 3, "Complete region/guards required")
    data = []
    expected = [*B.GUARDS, (B.START, B.extent(source, target))]
    for transfer, (offset, length) in zip(observations[0], expected):
        value = private_file(path.parent / transfer["file"])
        require(type(transfer["offset"]) is int and transfer["offset"] == offset and
                type(transfer["bytes"]) is int and transfer["bytes"] == length == len(value) and
                transfer["sha256"] == digest(value), "Inspection extent/hash differs")
        data.append(value)
    B.guards(baseline, data[:2])
    B.images(source, target, data[2], bindings["interrupted"])
    statuses = report.get("status_checks", [])
    require(len(statuses) == 1 and statuses[0].get("event") == "flash_status_checked" and
            statuses[0].get("assessment", {}).get("software_preconditions_met") is True,
            "Permitted inspection status pair missing")
    return data[2], digest(raw)


def prepared(args):
    baseline, source, target, bindings = inputs(args)
    require(args.inspection is not None, "Completed inspection report required")
    current, report_sha = inspected(args.inspection, bindings, source, target, baseline)
    reviewed = {**bindings, "inspection_report_sha256": report_sha,
                "operation_plan": B.plan(source, target, current, args.interrupted)}
    return baseline, source, target, current, reviewed


def session(args, baseline, source, target, bindings, current=None, reviewed=None, raw_plan=None):
    require(args.mode in ("inspect", "execute"), "Explicit serial mode required")
    if args.mode == "execute":
        require(reviewed is not None and raw_plan is not None and
                digest(raw_plan) == args.expected_plan_sha256 and
                canonical(json.loads(raw_plan)) == canonical(reviewed),
                "Exact reviewed plan/hash required before serial access")
        require(canonical(reviewed.get("operation_plan")) == canonical(B.plan(source, target, current, args.interrupted)),
                "Operation differs before serial access")
        require(canonical({k: v for k, v in reviewed.items()
                           if k not in ("inspection_report_sha256", "operation_plan")}) == canonical(bindings),
                "Execution bindings differ before serial access")
    directory = R.output_directory(args.output)
    if raw_plan is not None:
        with (directory / "reviewed-plan.json").open("xb") as stream:
            stream.write(raw_plan)
            stream.flush()
            os.fsync(stream.fileno())
        C.sync_directory(directory)
    report = {"mode": args.mode, "status": "prepared", "start_utc": C.utc(),
              "bindings": bindings, "helper_upload_attempted": False, "baud": 115200,
              "observations": [], "status_checks": [], "application_boot_verified": False,
              "scope": "Application union and factory/other-bank guards only; unscanned flash preserved by write scope"}

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
        print(f"Development {args.mode}; region {B.extent(source, target)} bytes; no full-flash scans", flush=True)

        def acquire():
            number = len(report["observations"]) + 1
            observation, values = [], []
            for label, (offset, length) in zip(("factory", "other-bank", "application"),
                                             (*B.GUARDS, (B.START, B.extent(source, target)))):
                partial = directory / f"{label}-{number:02d}.bin.part"
                with partial.open("xb") as stream:
                    result = R.receive_flash(port, offset, length, stream)
                    stream.flush()
                    os.fsync(stream.fileno())
                data = partial.read_bytes()
                require(len(data) == length, "Incomplete region read")
                final = partial.with_suffix("")
                partial.rename(final)
                C.sync_directory(directory)
                observation.append({"file": final.name, "offset": offset,
                                    **result, "sha256": digest(data)})
                values.append(data)
            report["observations"].append(observation)
            save()
            print(f"Region acquisition {number} complete", flush=True)
            return values[2], values[:2]

        if args.mode == "inspect":
            data, protected = acquire()
            B.guards(baseline, protected)
            B.images(source, target, data, args.interrupted)
            T.check_status(port, B.START, B.START + len(data), "inspection",
                           lambda event, **fields: report["status_checks"].append({"event": event, **fields}))
            report["status"] = "complete"
        else:
            def progress(n, total):
                if n == 1 or n % 8 == 0 or n == total:
                    print(f"Verified changed sectors: {n}/{total}", flush=True)

            result = B.run(port, baseline, source, target, current, reviewed["operation_plan"],
                           acquire, directory / "transaction", progress, args.interrupted)
            report.update(status="verified", result=result, reviewed_plan_sha256=digest(raw_plan))
        print("Completed; application boot remains a separate bench check", flush=True)
        return 0
    except (Exception, KeyboardInterrupt) as error:
        report.update(status="stopped", error=f"{type(error).__name__}: {error}")
        print("Stopped without replay; keep application boot excluded and inspect evidence", flush=True)
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
    for name in ("baseline", "source", "target", "rom-report", "small-report", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--port", required=True)
    parser.add_argument("--inspection", type=Path)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--expected-plan-sha256")
    parser.add_argument("--interrupted", action="store_true",
                        help="Finish a stopped update: staged marker, every sector source, staged target or erased")
    args = parser.parse_args()
    os.umask(0o077)
    if args.mode == "inspect":
        baseline, source, target, bindings = inputs(args)
        return session(args, baseline, source, target, bindings)
    baseline, source, target, current, reviewed = prepared(args)
    if args.mode == "prepare":
        require(args.output.resolve().is_relative_to((ROOT / "private").resolve()), "Private plan required")
        raw = canonical(reviewed) + b"\n"
        with args.output.open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        C.sync_directory(args.output.parent)
        print("Prepared development plan SHA256:", digest(raw))
        return 0
    require(args.plan is not None and args.expected_plan_sha256 is not None, "Reviewed plan and hash required")
    raw = private_file(args.plan)
    require(digest(raw) == args.expected_plan_sha256 and
            canonical(json.loads(raw)) == canonical(reviewed), "Reviewed plan/source/evidence differs")
    bindings = {k: v for k, v in reviewed.items()
                if k not in ("inspection_report_sha256", "operation_plan")}
    return session(args, baseline, source, target, bindings, current, reviewed, raw)


if __name__ == "__main__":
    raise SystemExit(main())
