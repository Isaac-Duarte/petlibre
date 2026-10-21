#!/usr/bin/env python3
"""Region-scoped bank-1 development transactions; injected transport only.

Both complete source/target OTA images are explicit inputs. This is not an
inactive-bank OTA implementation or boot controller. Unchanged data after a
shorter target remains in place, outside its mapping.

`interrupted=True` finishes a stopped transaction of this kind from a fresh
acquisition, never from the stopped journal: the marker sector must already be
staged, and every sector must exactly equal its source, its staged target or
erased flash. One sector may instead hold the staged target's first 1-3
program frames followed by erased flash, the only state a stopped program of
an erased sector leaves. Anything else (a partial erase, another partial
program, other content) is refused.
"""
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


C = module("development_coordinator", "recovery-transaction.py")
T, R = C.SECTORS, C.SECTORS.READER
L = module("development_layout", "audit-ota-layout.py")
require, digest, canonical = C.require, C.digest, C.canonical
START, END, SECTOR = T.START, T.END, T.SECTOR
GUARDS = ((0, 0x6000), (0x6000, SECTOR))
FRAME = 1024  # Program frames per sector, written in order after its erase.
KIND = "plaf103_bank1_development_update"


def payload(image):
    require(type(image) is bytes, "Immutable complete OTA bytes required")
    data = L.container(image)
    require(0 < len(data) <= END - START and len(data) % SECTOR == 0,
            "Development image exceeds aligned bank-1 footprint")
    L.regions(data)  # Both cores, exact addresses, headers and padding.
    return data


def extent(source, target):
    return max(len(payload(source)), len(payload(target)))


def guards(baseline, observed):
    baseline = T.pinned_baseline(baseline)
    require(len(observed) == len(GUARDS), "Missing factory/other-bank guards")
    for (offset, length), data in zip(GUARDS, observed):
        require(type(data) is bytes and data == baseline[offset:offset + length],
                "Factory or other-bank guard differs; investigate before update")
    require(baseline[0x6000:0x6008] != baseline[START:START + 8],
            "Other bank must be invalid for this development path")


def images(source, target, current, interrupted=False):
    old, new = payload(source), payload(target)
    require(type(current) is bytes and len(current) == max(len(old), len(new)),
            "Exact source/target union acquisition required")
    if not interrupted:
        require(current[:len(old)] == old, "Current application is not the reviewed source image")
        proposed = new + current[len(new):]
        staged = b"\xff" * 8 + proposed[8:]
        return proposed, staged
    # The tail beyond the target is never written, so it must still be the source's.
    union = old + current[len(old):]
    proposed = new + union[len(new):]
    staged = b"\xff" * 8 + proposed[8:]
    require(current[:SECTOR] == staged[:SECTOR], "Interrupted update requires the staged marker sector")
    partial = []
    for at in range(0, len(current), SECTOR):
        sector, want = current[at:at + SECTOR], staged[at:at + SECTOR]
        if sector in (union[at:at + SECTOR], want, b"\xff" * SECTOR):
            continue
        frames = [want[:n] + b"\xff" * (SECTOR - n) for n in range(FRAME, SECTOR, FRAME)]
        require(sector in frames, f"Sector {START + at:#x} is neither source, staged target nor erased")
        partial.append(at)
    require(len(partial) <= 1, "More than one partially programmed sector")
    return proposed, staged


def plan(source, target, current, interrupted=False):
    proposed, staged = images(source, target, current, interrupted)
    changed = current != proposed
    offsets = [i for i in range(0, len(current), SECTOR)
               if current[i:i + SECTOR] != staged[i:i + SECTOR]] if changed else []
    require(interrupted or not changed or offsets[0] == 0, "Marker must be staged first")
    return {"schema_version": 1, "kind": KIND, "bank": 1, "interrupted": interrupted,
            "source_ota_sha256": digest(source), "target_ota_sha256": digest(target),
            "source_payload_bytes": len(payload(source)), "target_payload_bytes": len(payload(target)),
            "region": {"offset": START, "bytes": len(current)},
            "current_region_sha256": digest(current), "staged_region_sha256": digest(staged),
            "proposed_region_sha256": digest(proposed),
            "ordered_sectors": [{"offset": START + i, "bytes": SECTOR,
                                 "before_sha256": digest(current[i:i + SECTOR]),
                                 "staged_sha256": digest(staged[i:i + SECTOR])} for i in offsets],
            "erase_sectors": len(offsets), "staging_packets": len(offsets) * 4,
            "commit_packets": int(changed), "additional_commit_erases": 0,
            "retained_tail_sha256": digest(current[len(payload(target)):]),
            "scope": "Application union and factory/other-bank guards read back; other flash is not scanned"}


def run(port, baseline, source, target, current, reviewed, acquire, output, progress=None, interrupted=False):
    """acquire returns (application union, factory/other-bank guard byte arrays)."""
    expected = plan(source, target, current, interrupted)
    require(canonical(reviewed) == canonical(expected), "Reviewed development plan differs")
    require(callable(acquire), "Region acquisition callback required")
    proposed, staged = images(source, target, current, interrupted)
    # Freeze all packets before creating durable intent or talking to transport.
    sectors = []
    for entry in expected["ordered_sectors"]:
        offset = entry["offset"]
        at = offset - START
        data = staged[at:at + SECTOR]
        frames = tuple(T.program_frame(n + 1, offset + n * 1024, data[n * 1024:(n + 1) * 1024])
                       for n in range(4))
        sectors.append((offset, data, frames))
    commit = T.program_frame(1, START, proposed[:1024])
    journal = C.Journal(output)

    def record(event, **fields):
        journal.append({"event": event, **fields})

    def check(expected_bytes, phase):
        data, protected = acquire()
        guards(baseline, protected)
        require(data == expected_bytes, phase + " application region differs")
        record("region_verified", phase=phase, sha256=digest(data), bytes=len(data))

    try:
        record("transaction_start", plan=expected)
        check(current, "preimage")
        for number, (offset, data, frames) in enumerate(sectors, 1):
            at = offset - START
            require(T.read_sector(port, offset) == current[at:at + SECTOR], "Sector preimage drift")

            def sector_record(event, **fields):
                record(event, offset=offset, **fields)

            T.check_status(port, offset, offset + SECTOR, "before_erase", sector_record)
            sector_record("erase_intent", bytes=SECTOR)
            T.command_ready(port)  # After journal fsync: no stale idle-ready byte.
            R.send(port, b"\x17" + offset.to_bytes(3, "little") + b"\x01\x00")
            R.expect(port, R.ACK)
            require(T.read_sector(port, offset) == b"\xff" * SECTOR, "Erase readback differs")
            sector_record("erase_verified")
            T.check_status(port, offset, offset + SECTOR, "before_program", sector_record)
            T.program_packets(port, frames, sector_record)
            require(T.read_sector(port, offset) == data, "Program readback differs")
            sector_record("sector_staged", sha256=digest(data))
            if progress:
                progress(number, len(sectors))
        if sectors or interrupted:
            check(staged, "staged")
            require(T.read_sector(port, START) == staged[:SECTOR], "Marker sector drift")

            def commit_record(event, **fields):
                record(event, offset=START, phase="marker_commit", **fields)

            T.check_status(port, START, START + 1024, "before_marker_commit", commit_record)
            commit_record("marker_commit_intent", target_region_sha256=digest(proposed))
            T.program_packets(port, (commit,), commit_record)
            require(T.read_sector(port, START) == proposed[:SECTOR], "Committed marker readback differs")
            check(proposed, "final")
            record("marker_commit_verified", sha256=digest(proposed))
        record("transaction_verified", snapshot_sha256=digest(proposed), erase_sectors=len(sectors))
        report = {"status": "verified_via_supplied_transports", "region_sha256": digest(proposed),
                  "erase_sectors": len(sectors), "application_boot_verified": False,
                  "scope": expected["scope"]}
        journal.save_report(report)
        return report
    except (Exception, KeyboardInterrupt) as error:
        try:
            record("transaction_stopped", error_type=type(error).__name__,
                   mutation_possible=journal.mutation_possible)
            journal.save_report({"status": "stopped", "error_type": type(error).__name__,
                                 "mutation_possible": journal.mutation_possible})
        except Exception:
            pass
        raise
    finally:
        journal.close()
