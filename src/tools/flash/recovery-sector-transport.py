#!/usr/bin/env python3
"""Bounded stock-sector transport component, tested only with in-memory peers.

No CLI, serial import, port opening, helper upload, reset or status-register
write. Fixed status reads guard command-mode mutation boundaries. A supervised
coordinator must supply the verified helper transport, same-unit/fresh-snapshot
evidence and durable journal.
"""
import hashlib
import importlib.util
import io
from pathlib import Path
import re
import struct

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("uart_reader", ROOT / "src/tools/flash/uart-flash-read.py")
READER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(READER)
STATUS_SPEC = importlib.util.spec_from_file_location("flash_status", ROOT / "src/tools/flash/flash-status.py")
STATUS = importlib.util.module_from_spec(STATUS_SPEC)
STATUS_SPEC.loader.exec_module(STATUS)
BASELINE_SHA = "2751e6c870af7312fe1ff0039831ac5799fb215d629d85af8720b6e50a7479bc"
START, END, SECTOR = 0x186000, 0x2D6000, 4096


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def program_frame(number, offset, payload):
    require(type(number) is int and 1 <= number <= 4, "Packet number must be 1–4")
    require(type(offset) is int and START <= offset and offset + 1024 <= END
            and offset % 1024 == 0, "Packet outside aligned stock bank 1")
    require(isinstance(payload, bytes) and len(payload) == 1024, "Full immutable 1 KiB payload required")
    body = struct.pack("<I", 0x08000000 + offset) + payload
    return bytes([2, number, number ^ 255]) + body + bytes([sum(body) & 255])


def command_ready(port):
    READER.wait_naks(port)
    pending = port.in_waiting
    if pending:
        require(port.read(pending) == b"\x15" * pending, "Unexpected buffered helper output")


def read_sector(port, offset):
    output = io.BytesIO()
    READER.receive_flash(port, offset, SECTOR, output)
    result = output.getvalue()
    require(len(result) == SECTOR, "Incomplete sector read")
    return result


def pinned_baseline(baseline):
    baseline = bytes(baseline)
    require(len(baseline) == 0x800000 and digest(baseline) == BASELINE_SHA, "Pinned full baseline required")
    return baseline


def sector_payload(baseline, offset):
    """Keep the preferred bank unbootable until a separately verified commit."""
    payload = bytearray(baseline[offset:offset + SECTOR])
    if offset == START:
        payload[:8] = b"\xff" * 8
    return bytes(payload)


def check_status(port, start, end, boundary, record):
    """Two matching sequential reads are required; no poll/retry or status writes."""
    first = STATUS.read_status_registers(port)
    second = STATUS.read_status_registers(port)
    if first != second:
        record("flash_status_unstable", boundary=boundary, start=start, end_exclusive=end,
               first=list(first), second=list(second))
        raise ValueError("Flash status changed between observations; mutation refused")
    assessed = STATUS.assess_range(*second, start, end)
    record("flash_status_checked", boundary=boundary, registers=list(second), assessment=assessed)
    require(assessed["software_preconditions_met"], "Flash status blocks " + boundary + ": "
            + ", ".join(assessed["blocking_reasons"]))
    return assessed


def program_packets(port, frames, record):
    command_ready(port)
    READER.baudcheck(port)
    for number, frame in enumerate(frames, 1):
        record("program_packet_intent", sequence=number, frame_sha256=digest(frame))
        READER.send(port, frame)
        READER.expect(port, READER.ACK)
        record("program_packet_ack", sequence=number)
    record("program_eot_intent")
    READER.send(port, b"\x04")
    READER.expect(port, READER.ACK)


def restore_sector(port, baseline, offset, expected_before_sha256, journal):
    """Restore only this pinned unit's original bank-1 sector; never retry.

    Every mutation intent is journaled before transmission. A NAK/timeout may
    follow a mutation and is always terminal here. This function does not prove
    physical entry or same-unit identity. The first sector's marker is staged as
    0xff; commit_marker separately verifies full-image/range preconditions.
    """
    baseline = pinned_baseline(baseline)
    require(type(offset) is int and START <= offset and offset + SECTOR <= END
            and offset % SECTOR == 0, "Sector outside aligned stock bank 1")
    require(isinstance(expected_before_sha256, str) and
            re.fullmatch(r"[0-9a-f]{64}", expected_before_sha256), "Expected before-sector hash required")
    require(callable(journal), "Mutation journal callback required")
    payload = sector_payload(baseline, offset)
    expected_after = digest(payload)
    frames = [program_frame(i + 1, offset + i * 1024, payload[i * 1024:(i + 1) * 1024])
              for i in range(4)]
    mutation_possible = False

    def record(event, **fields):
        journal({"event": event, "offset": offset, **fields})

    try:
        before = read_sector(port, offset)
        require(digest(before) == expected_before_sha256, "Current sector differs from reviewed before hash")
        if offset == START and before == payload:
            record("marker_already_staged", sha256=expected_after)
            return {"offset": offset, "bytes": SECTOR, "before_sha256": expected_before_sha256,
                    "after_sha256": expected_after, "program_packets": 0, "marker_withheld": True}
        require(before != payload, "Sector already matches baseline; no erase required")
        if offset == START:
            require(before[:8] != baseline[START:START + 8], "Target marker is still valid; stop for plan review")
        record("before_verified", sha256=expected_before_sha256)
        check_status(port, offset, offset + SECTOR, "before_erase", record)
        record("erase_intent", bytes=SECTOR, after_sha256=expected_after)
        # Durable storage can stall beyond the helper's 50 ms ready interval.
        # Drain idle bytes after that stall, immediately before transmission.
        command_ready(port)
        mutation_possible = True
        READER.send(port, b"\x17" + offset.to_bytes(3, "little") + b"\x01\x00")
        READER.expect(port, READER.ACK)
        erased = read_sector(port, offset)
        require(erased == b"\xff" * SECTOR, "Erase readback is not all 0xff; no program attempted")
        record("erase_readback_verified")
        check_status(port, offset, offset + SECTOR, "before_program", record)
        program_packets(port, frames, record)
        after = read_sector(port, offset)
        require(after == payload, "Program readback differs from exact baseline sector")
        record("sector_verified", sha256=expected_after)
        return {"offset": offset, "bytes": SECTOR, "before_sha256": expected_before_sha256,
                "after_sha256": expected_after, "program_packets": 4, "marker_withheld": offset == START}
    except (Exception, KeyboardInterrupt) as error:
        try:
            record("stopped", mutation_possible=mutation_possible, error_type=type(error).__name__)
        except Exception:
            pass  # Preserve the first failure, including a failed journal callback.
        raise


def commit_marker(port, baseline, current_snapshot, snapshot_provider, journal):
    """Commit the eight marker bytes after exact staged-image/range verification.

    The helper's selected wrapper does not erase. Reprogram the first full 1 KiB
    chunk with baseline bytes: only the withheld marker needs 1-to-0 transitions.
    All other image bytes must already match, including the rest of that chunk.
    Snapshot freshness and physical source identity remain caller responsibilities.
    """
    baseline = pinned_baseline(baseline)
    current = bytes(current_snapshot)
    require(len(current) == len(baseline), "Exact current full snapshot required")
    require(callable(snapshot_provider) and callable(journal), "Snapshot provider and journal required")
    proposed = bytearray(current)
    proposed[START:END] = baseline[START:END]
    staged = bytearray(proposed)
    staged[START:START + 8] = b"\xff" * 8
    staged_sector = bytes(staged[START:START + SECTOR])
    mutation_possible = False

    def record(event, **fields):
        journal({"event": event, "offset": START, "phase": "marker_commit", **fields})

    def snapshot():
        data = snapshot_provider()
        require(isinstance(data, (bytes, bytearray)) and len(data) == len(baseline),
                "Snapshot provider must return exactly 8 MiB")
        return bytes(data)

    try:
        require(snapshot() == staged, "Full staged image or preserved ranges differ; marker remains uncommitted")
        require(read_sector(port, START) == staged_sector, "Marker sector changed after full verification")
        record("marker_preconditions_verified", staged_snapshot_sha256=digest(staged))
        check_status(port, START, START + 1024, "before_marker_commit", record)
        frame = program_frame(1, START, baseline[START:START + 1024])
        record("marker_commit_intent", proposed_snapshot_sha256=digest(proposed))
        mutation_possible = True
        program_packets(port, [frame], record)
        require(read_sector(port, START) == baseline[START:START + SECTOR], "Committed marker-sector readback differs")
        after = snapshot()
        require(after == proposed, "Final image or preserved ranges differ after marker commit")
        record("marker_commit_verified", snapshot_sha256=digest(after))
        return {"snapshot_sha256": digest(after), "marker_bytes": 8, "program_packets": 1,
                "additional_erases": 0}
    except (Exception, KeyboardInterrupt) as error:
        try:
            record("stopped", mutation_possible=mutation_possible, error_type=type(error).__name__)
        except Exception:
            pass
        raise
