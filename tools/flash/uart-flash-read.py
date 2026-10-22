#!/usr/bin/env python3
"""Restricted PLAF103 UART inspection/read client; never programs or erases flash.

The vendor helper executes in RAM and changes controller state, including OTF
decryption. Its inclusion here does not certify actual ROM or recovery safety.
All captures and reports must be new directories under ignored private/.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import struct
import time

ROOT = Path(__file__).resolve().parents[2]
HELPER_SHA256 = "9307121385cb390dfd2da64da2c6c515f17b5a9556b3d04021487c9b9f220b55"
HELPER_BYTES = 4688
HELPER_ADDRESS = 0x82000
CAPACITY = 8 * 1024 * 1024
ACK, NAK, EOT, STX, CAN = 6, 21, 4, 2, 24
ROM_POINTS = (0x1298, 0x0474, 0x1434, 0x14C0, 0x1504, 0x2E9C,
              0x3594, 0x40B0, 0x43D4, 0x61A0, 0x61D4, 0x6220,
              0x6250, 0x6274, 0x628C, 0x6590, 0x66AC)
ROM_SHA256 = {
    "301be574cc338a0bb0a3516f691ec4409064be39fc4144ebfa46488ede9e1612",
    "db0fdd8b2232a0d9edaa403715a9992ae36944fbda4dd936e017c1fa531e66d6",
}


class ProtocolError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise ProtocolError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def utc():
    return datetime.now(timezone.utc).isoformat()


def load_helper(path):
    data = Path(path).read_bytes()
    require(len(data) == HELPER_BYTES and sha256(data) == HELPER_SHA256,
            "Helper does not match the reviewed vendor binary")
    require(struct.unpack_from("<I", data)[0] == HELPER_ADDRESS + 0x21,
            "Unexpected helper entry pointer")
    return data


def read_request(offset, length):
    require(isinstance(offset, int) and isinstance(length, int), "Integer range required")
    require(offset >= 0 and length > 0 and offset % 4096 == length % 4096 == 0,
            "Read range must be positive and aligned to 4 KiB")
    require(offset + length <= CAPACITY, "Read exceeds the identified flash capacity")
    return bytes([0x20]) + offset.to_bytes(3, "little") + (length // 4096).to_bytes(2, "little")


def upload_frame(helper, index):
    require(len(helper) == HELPER_BYTES and sha256(helper) == HELPER_SHA256,
            "Only the pinned helper may be uploaded")
    require(0 <= index < (len(helper) + 1023) // 1024, "Invalid helper packet index")
    payload = helper[index * 1024:(index + 1) * 1024].ljust(1024, b"\xff")
    address = struct.pack("<I", HELPER_ADDRESS + index * 1024)
    body = address + payload
    packet = index + 1
    return bytes([STX, packet, packet ^ 0xFF]) + body + bytes([sum(body) & 0xFF])


def exact_read(port, count, timeout=3):
    deadline = time.monotonic() + timeout
    data = bytearray()
    while len(data) < count:
        require(time.monotonic() < deadline, "Serial read deadline expired")
        chunk = port.read(count - len(data))
        require(len(chunk) <= count - len(data), "Transport returned too many bytes")
        data.extend(chunk)
    return bytes(data)


def send(port, data):
    require(port.write(data) == len(data), "Partial serial write; aborting without replay")
    port.flush()


def expect(port, value, timeout=3):
    actual = exact_read(port, 1, timeout)[0]
    require(actual == value, f"Unexpected protocol byte {actual:#04x}; expected {value:#04x}")


def wait_naks(port, count=3, timeout=5):
    """Require consecutive NAKs; retain any preceding banner in the transport log."""
    deadline = time.monotonic() + timeout
    seen = 0
    while seen < count:
        require(time.monotonic() < deadline, "Loader NAK deadline expired")
        chunk = port.read(1)
        if not chunk:
            continue
        seen = seen + 1 if chunk == bytes([NAK]) else 0


def baudcheck(port):
    """One request; tolerate only already-in-flight NAKs before its ACK."""
    send(port, b"\x07")
    deadline = time.monotonic() + 3
    while True:
        require(time.monotonic() < deadline, "BAUDCHK acknowledgement deadline expired")
        value = exact_read(port, 1, 1)[0]
        if value == ACK:
            return
        require(value == NAK, "Unexpected BAUDCHK response")


def upload_helper(port, helper):
    """No baud changes, flash addressing, cancellation, retries or fallback images."""
    require(len(helper) == HELPER_BYTES and sha256(helper) == HELPER_SHA256,
            "Only the pinned helper may be uploaded")
    wait_naks(port)
    # Drain only already-buffered NAK bytes before the ROM BAUDCHK exchange.
    pending = getattr(port, "in_waiting", 0)
    if pending:
        require(port.read(pending) == bytes([NAK]) * pending, "Unexpected pending loader output")
    baudcheck(port)
    for index in range((len(helper) + 1023) // 1024):
        send(port, upload_frame(helper, index))
        expect(port, ACK)
    send(port, bytes([EOT]))
    expect(port, ACK)
    wait_naks(port)


def receive_flash(port, offset, length, output, progress=None):
    """Exact checksum-framed read; malformed, partial or cancelled reads fail closed.

    A bounded duplicate of the previous packet is accepted only with identical
    payload (lost ACK). No requests or damaged packets are automatically retried.
    The file remains an incomplete .part until exact length and EOT are verified.
    """
    request = read_request(offset, length)
    wait_naks(port)
    pending = getattr(port, "in_waiting", 0)
    if pending:
        require(port.read(pending) == bytes([NAK]) * pending, "Unexpected pending helper output")
    send(port, request)
    total = 0
    sequence = 1
    previous = None
    duplicates = 0
    deadline = time.monotonic() + 30 + length / 4000
    while True:
        require(time.monotonic() < deadline, "Overall read deadline expired")
        control = exact_read(port, 1, 5)[0]
        if control == EOT:
            require(total == length, "Premature EOT; incomplete read")
            send(port, bytes([ACK]))
            wait_naks(port)
            return {"bytes": total, "packets": total // 1024, "duplicate_packets": duplicates}
        require(control == STX, f"Unexpected read control {control:#04x}; no fallback")
        frame = exact_read(port, 1027, 3)
        number, complement = frame[:2]
        payload = frame[2:-1]
        require(number ^ complement == 0xFF, "Packet number/complement mismatch")
        require(sum(payload) & 0xFF == frame[-1], "Read packet checksum mismatch")
        if number == ((sequence - 1) & 0xFF) and previous is not None:
            require(payload == previous and duplicates < 8, "Changed or excessive duplicate packet")
            duplicates += 1
            send(port, bytes([ACK]))
            continue
        require(number == sequence & 0xFF, "Out-of-order read packet")
        require(total + len(payload) <= length, "Read exceeded requested length")
        output.write(payload)
        total += len(payload)
        previous = payload
        sequence += 1
        send(port, bytes([ACK]))
        if progress and (total % (256 * 1024) == 0 or total == length):
            progress(total, length)


def collect(port, seconds):
    deadline = time.monotonic() + seconds
    result = bytearray()
    while time.monotonic() < deadline:
        result.extend(port.read(4096))
        require(len(result) <= 512 * 1024, "Unexpectedly large console response")
    return bytes(result)


def observe(port, seconds):
    """Receive only: NAKs are observations, never an automatic loader assertion."""
    require(1 <= seconds <= 180, "Observation must be 1–180 seconds")
    data = collect(port, seconds)
    return {"bytes": len(data), "transmitted_bytes": 0, "nak_bytes": data.count(bytes([NAK])),
            "longest_nak_run": max((len(run) for run in re.findall(b"\x15+", data)), default=0),
            "download_banner_seen": b"#Flash Download Start" in data,
            "stock_startup_markers": data.count(b"app_main:version:"),
            "fault_marker_seen": any(s in data for s in (b"Hard Fault", b"HardFault", b"Fault on thread")),
            "limit": "Serial output only; operator must establish power/strap state and loader identity"}


def console_query(port, command, seconds=1):
    require(command in (b"?\r\n", b">>\r\n") or
            re.fullmatch(rb"DW 0000[0-9A-F]{4}\r\n", command),
            "Console command is outside the fixed ROM-inspection profile")
    send(port, command)
    reply = collect(port, seconds)
    require(not any(marker in reply for marker in
                    (b"Hard Fault", b"HardFault", b"Fault on thread", b"app_main:version:")),
            "Fault or startup observed during console query; stop and inspect capture")
    return reply


def is_km0_help(reply):
    return all(marker in reply for marker in
               (b"TEST COMMAND MODE HELP 12", b"switch shell to KM4", b"HSSDM32K",
                b"CTC", b"FWDBG", b"TEST COMMAND MODE END  c")) and b"switch shell to KM0" not in reply


def inspect_rom(port, candidates, start_core="km4"):
    """Enter KM0 only after verifying both actual help-table identities."""
    initial = collect(port, .5)
    require(b"Hard Fault" not in initial, "Unexpected pre-existing fault output")
    require(start_core in ("km4", "km0"), "Explicit known starting core required")
    if start_core == "km4":
        high = console_query(port, b"?\r\n")
        require(b"switch shell to KM0" in high and b"0e11df78" in high,
                "Expected stock KM4 command table not observed; no core switch attempted")
        console_query(port, b">>\r\n", .5)
    low = console_query(port, b"?\r\n")
    require(is_km0_help(low),
            "Expected stock KM0 help table not observed; no ROM read attempted")
    samples = []
    surviving = dict(candidates)
    for base in ROM_POINTS:
        for address in range(base & ~3, (base & ~3) + 16, 4):
            response = console_query(port, f"DW {address:08X}\r\n".encode(), .25)
            values = re.findall(f"{address:08X}:\\s*([0-9A-Fa-f]{{8}})".encode(), response)
            require(len(values) == 1, "Expected exactly one matching ROM word; no retry")
            value = int(values[0], 16)
            surviving = {name: candidate for name, candidate in surviving.items()
                         if candidate[address:address + 4] == struct.pack("<I", value)}
            require(surviving,
                    f"ROM differs from candidate at {address:#x}; helper upload blocked")
            samples.append({"address": f"0x{address:08x}", "value": f"0x{value:08x}"})
    console_query(port, b">>\r\n", .5)
    high = console_query(port, b"?\r\n")
    require(b"switch shell to KM0" in high and b"0e11df78" in high,
            "KM4 console return not verified")
    return {"sampled_words": samples,
            "matching_candidates": {name: sha256(candidate) for name, candidate in surviving.items()},
            "returned_to_km4": True,
            "limit": "Sampled ROM identity only; not whole-ROM or all-side-effect verification"}


def validate_rom_report(path):
    path = Path(path)
    require(path.resolve().is_relative_to((ROOT / "private").resolve()),
            "ROM report must refer to private evidence")
    report = json.loads(path.read_text())
    result = report.get("result", {})
    require(report.get("status") == "complete" and report.get("mode") == "inspect-rom" and
            result.get("returned_to_km4") is True, "Successful ROM inspection required")
    candidates = result.get("matching_candidates", {})
    require(candidates and set(candidates.values()) <= ROM_SHA256,
            "ROM report does not identify a pinned candidate")
    expected = [f"0x{a:08x}" for base in ROM_POINTS
                for a in range(base & ~3, (base & ~3) + 16, 4)]
    require([word.get("address") for word in result.get("sampled_words", [])] == expected,
            "Incomplete ROM sample set")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(sha256((path.parent / name).read_bytes()) == report.get(name + "_sha256"),
                "ROM serial evidence hash mismatch")
    return report


def validate_status_session(path, port):
    """Bind continuation to a completed bounded read and its raw evidence.

    This verifies saved evidence, not live helper identity or power continuity;
    the supervised caller must establish those before invoking read-status.
    """
    require(path is not None, "A completed bounded-read report is required")
    path = Path(path)
    require(path.resolve().is_relative_to((ROOT / "private").resolve()),
            "Bounded-read report must refer to private evidence")
    report = json.loads(path.read_text())
    require(report.get("status") == "complete" and report.get("mode") == "read-small" and
            report.get("port") == port and report.get("helper_sha256") == HELPER_SHA256 and
            report.get("helper_upload_completed") is True and
            report.get("result", {}).get("bytes") == 4096 and
            report["result"].get("offset") == 0, "Bounded-read session does not validate")
    data = (path.parent / "flash-read.bin").read_bytes()
    require(len(data) == 4096 and sha256(data) == report["result"].get("sha256"),
            "Bounded-read image does not validate")
    for name in ("serial-rx.bin", "serial-tx.bin"):
        require(sha256((path.parent / name).read_bytes()) == report.get(name + "_sha256"),
                "Bounded-read serial evidence hash mismatch")
    return report


def inspect_status(port, record):
    """Two fixed status observations; preserve each before checking stability."""
    spec = importlib.util.spec_from_file_location(
        "flash_status", Path(__file__).resolve().parent / "flash-status.py")
    status = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(status)
    observations = []
    for _ in range(2):
        values = status.read_status_registers(port)
        observations.append(list(values))
        record(list(values))
    require(observations[0] == observations[1], "Status observations differ; stop without retry")
    return {"observations": observations, "matching": True,
            "bank1_assessment": status.assess_range(*observations[1], 0x186000, 0x2D6000),
            "status_write_commands": False,
            "limit": "Sequential read observations only; physical write permission not established"}


class EvidencePort:
    def __init__(self, serial_port, directory):
        self.port = serial_port
        self.rx = (directory / "serial-rx.bin").open("xb")
        self.tx = (directory / "serial-tx.bin").open("xb")
        self.events = (directory / "serial-events.jsonl").open("x")
        self.offsets = {"rx": 0, "tx": 0}

    @property
    def in_waiting(self):
        return self.port.in_waiting

    def record(self, direction, data):
        if data:
            stream = self.rx if direction == "rx" else self.tx
            stream.write(data)
            stream.flush()
            self.events.write(json.dumps({"utc": utc(), "direction": direction,
                                         "offset": self.offsets[direction], "bytes": len(data)}) + "\n")
            self.events.flush()
            self.offsets[direction] += len(data)

    def read(self, count):
        data = self.port.read(count)
        self.record("rx", data)
        return data

    def write(self, data):
        # Journal intent before any transmission; no automatic command replay.
        self.record("tx", data)
        return self.port.write(data)

    def flush(self):
        self.port.flush()

    def close(self):
        self.port.close()
        for stream in (self.rx, self.tx, self.events):
            stream.close()


def output_directory(path):
    private = (ROOT / "private").resolve()
    path = Path(path).absolute()
    require(not path.is_symlink() and path.resolve().is_relative_to(private),
            "Output must be a new private directory")
    path.mkdir(parents=True, exist_ok=False, mode=0o700)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("observe", "inspect-rom", "read-small", "read-full", "read-status"))
    parser.add_argument("--port", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--helper", type=Path, default=ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin")
    parser.add_argument("--candidate-rom", type=Path, action="append")
    parser.add_argument("--rom-report", type=Path)
    parser.add_argument("--small-report", type=Path)
    parser.add_argument("--seconds", type=float, default=60, help="Receive-only observation duration (1–180 seconds)")
    parser.add_argument("--start-core", choices=("km4", "km0"), default="km4",
                        help="Known console owner for ROM inspection; never inferred on a mismatch")
    args = parser.parse_args()
    # Validate local inputs before opening any port or writing any command.
    helper = None
    if args.mode == "observe":
        require(1 <= args.seconds <= 180, "Observation must be 1–180 seconds")
    elif args.mode == "inspect-rom":
        require(args.candidate_rom is not None, "Explicit candidate ROM is required")
        candidates = {str(path): path.read_bytes() for path in args.candidate_rom}
        require(all(len(data) == 54144 and sha256(data) in ROM_SHA256
                    for data in candidates.values()), "Candidate is not a pinned SDK ROM")
    else:
        helper = load_helper(args.helper)
        require(args.rom_report is not None, "A reviewed successful ROM inspection report is required")
        validate_rom_report(args.rom_report)
        if args.mode == "read-status":
            validate_status_session(args.small_report, args.port)
        if args.mode == "read-full":
            require(args.small_report is not None, "A completed bounded read is required first")
            small = json.loads(args.small_report.read_text())
            require(args.small_report.resolve().is_relative_to((ROOT / "private").resolve()) and
                    small.get("status") == "complete" and small.get("mode") == "read-small" and
                    small.get("helper_sha256") == HELPER_SHA256 and
                    small.get("helper_upload_completed") is True and
                    small.get("port") == args.port and
                    small.get("result", {}).get("bytes") == 4096 and
                    sha256((args.small_report.parent / "flash-read.bin").read_bytes()) ==
                    small["result"].get("sha256"), "Bounded-read evidence does not validate")
    os.umask(0o077)
    directory = output_directory(args.output)
    metadata = {"mode": args.mode, "port": args.port, "baud": 115200, "format": "8N1",
                "start_utc": utc(), "status": "prepared", "helper_sha256": sha256(helper) if helper else None,
                "flash_write_or_erase_commands": False, "capacity_bytes": CAPACITY,
                "limitations": ["No failed-boot recovery established", "Actual ROM side effects not exhaustively verified",
                                "Helper disables OTF; a raw dump may contain ciphertext", "External flash excludes separate OTP/eFuse"]}
    def save():
        (directory / "report.json").write_text(json.dumps(metadata, indent=2) + "\n")
    save()
    port = None
    try:
        import serial
        serial_port = serial.Serial(port=None, baudrate=115200, timeout=.05, write_timeout=3,
                                    xonxoff=False, rtscts=False, dsrdtr=False, exclusive=True)
        serial_port.dtr = False
        serial_port.rts = False
        serial_port.port = args.port
        serial_port.open()
        port = EvidencePort(serial_port, directory)
        metadata["status"] = "running"
        save()
        print(f"Recording {args.mode} in {directory}", flush=True)
        if args.mode == "observe":
            result = observe(port, args.seconds)
        elif args.mode == "inspect-rom":
            result = inspect_rom(port, candidates, args.start_core)
        elif args.mode == "read-status":
            metadata["helper_upload_attempted"] = False
            metadata["continuing_helper_session"] = str(args.small_report)
            metadata["bounded_read_report_sha256"] = sha256(args.small_report.read_bytes())
            metadata["status_observations"] = []
            save()

            def record_status(values):
                metadata["status_observations"].append(values)
                save()

            result = inspect_status(port, record_status)
        else:
            if args.mode == "read-small":
                # Require the already-observed ROM loader; never auto-enter it.
                metadata["helper_upload_attempted"] = True
                save()
                upload_helper(port, helper)
                metadata["helper_upload_completed"] = True
            else:
                # Continue the bounded-read helper session. Uploading over the
                # currently executing helper would overwrite its own RAM code.
                metadata["helper_upload_attempted"] = False
                metadata["continuing_helper_session"] = str(args.small_report)
                metadata["bounded_read_report_sha256"] = sha256(args.small_report.read_bytes())
            save()
            length = 4096 if args.mode == "read-small" else CAPACITY
            partial = directory / "flash-read.bin.part"
            with partial.open("xb") as output:
                result = receive_flash(port, 0, length, output,
                                       lambda done, total: print(f"Read {done}/{total} bytes", flush=True))
                output.flush()
                os.fsync(output.fileno())
            require(partial.stat().st_size == length, "Final file length mismatch")
            final = directory / "flash-read.bin"
            require(not final.exists(), "Refusing to overwrite a completed read")
            partial.rename(final)
            result["sha256"] = sha256(final.read_bytes())
            result["offset"] = 0
        metadata["result"] = result
        metadata["status"] = "complete"
        print("Completed; evidence retained privately", flush=True)
    except (OSError, ValueError, ImportError, KeyboardInterrupt) as error:
        metadata["status"] = "failed"
        metadata["error"] = f"{type(error).__name__}: {error}"
        print("Stopped; inspect private report and capture. No automatic retry or fallback.", flush=True)
    finally:
        if port:
            port.close()
        metadata["end_utc"] = utc()
        for name in ("serial-rx.bin", "serial-tx.bin"):
            path = directory / name
            if path.exists():
                metadata[name + "_sha256"] = sha256(path.read_bytes())
        save()
    return 0 if metadata["status"] == "complete" else 1


if __name__ == "__main__":
    raise SystemExit(main())
