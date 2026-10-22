#!/usr/bin/env python3
"""Region-scoped bank development transactions; injected transport only.

Bank 1 (`0x186000`, the factory loader's preferred bank) is the default.
Bank 0 (`0x6000`, the loader's fallback) uses its own transaction kind and
layout. Both refuse the factory region `0x0-0x6000`, which is only ever read
and compared with the pinned baseline.

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
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


C = module("development_coordinator", "flash/recovery-transaction.py")
T, R = C.SECTORS, C.SECTORS.READER
L = module("development_layout", "audit/audit-ota-layout.py")
require, digest, canonical = C.require, C.digest, C.canonical
START, END, SECTOR = T.START, T.END, T.SECTOR
FRAME = 1024  # Program frames per sector, written in order after its erase.
KIND = "plaf103_bank1_development_update"
FACTORY = (0, 0x6000)
MARKER = b"81958711"


class Layout(NamedTuple):
    """One bank's region, and the other bank's first sector read as a guard."""
    kind: str
    bank: int
    start: int
    end: int
    other: tuple


BANK1 = Layout(KIND, 1, START, END, (0x6000, SECTOR))
# Bank 0 keeps the reviewed stock footprint (0x150000), like bank 1.
BANK0 = Layout("plaf103_bank0_development_update", 0, 0x6000, 0x156000, (START, SECTOR))
LAYOUTS = {0: BANK0, 1: BANK1}
GUARDS = (FACTORY, BANK1.other)


def payload(image, layout=BANK1):
    require(type(image) is bytes, "Immutable complete OTA bytes required")
    data = L.container(image)
    require(0 < len(data) <= layout.end - layout.start and len(data) % SECTOR == 0,
            f"Development image exceeds aligned bank-{layout.bank} footprint")
    L.regions(data)  # Both cores, exact addresses, headers and padding.
    return data


def extent(source, target, layout=BANK1):
    """`source` None: bank 0's overwrite-an-invalid-bank mode covers the target only."""
    if source is None:
        require(layout.bank == 0, "Only bank 0 may be overwritten without a source review")
        return len(payload(target, layout))
    return max(len(payload(source, layout)), len(payload(target, layout)))


def guards(baseline, observed, layout=BANK1, other_sha256=None):
    """The factory region must equal the pinned baseline. The other bank's
    first sector must be in a known state and, once a plan pins it, stay
    byte-identical for the whole transaction.

    Bank 1 path: bank 0 holds either the pinned baseline (an invalidated
    marker) or a valid image. A valid bank 0 is the loader's fallback, so an
    interrupted bank-1 update then boots it instead of waiting forever.
    Bank 0 path: bank 1 must be valid, so the unit always has a bootable
    bank while bank 0's marker is withheld.
    """
    baseline = T.pinned_baseline(baseline)
    require(len(observed) == 2 and all(type(d) is bytes for d in observed), "Missing factory/other-bank guards")
    factory, other = observed
    require(factory == baseline[FACTORY[0]:FACTORY[0] + FACTORY[1]], "Factory region differs from the pinned baseline")
    at, length = layout.other
    require(len(other) == length, "Other-bank guard has the wrong length")
    if layout.bank == 1:
        require(other == baseline[at:at + length] or other[:8] == MARKER,
                "Bank 0 is neither the pinned baseline nor a valid image; investigate before update")
    else:
        require(other[:8] == MARKER, "Bank 1 must be valid while bank 0 is written")
    require(other_sha256 is None or digest(other) == other_sha256, "Other bank changed since inspection")


def images(source, target, current, interrupted=False, layout=BANK1, original=None, restore=False):
    """`source` None (bank 0 only): overwrite bank 0, whose contents must still
    be `original` (its bytes in a pinned full-flash read) with an invalid marker."""
    if source is None:
        require(layout.bank == 0, "Only bank 0 may be overwritten without a source review")
        new = payload(target, layout)
        require(type(original) is bytes and len(original) == len(new), "Pinned original bank-0 region required")
        require(original[:8] != MARKER, "The original bank 0 is valid; review it as the source instead")
        old = original
    else:
        old, new = payload(source, layout), payload(target, layout)
    require(type(current) is bytes and len(current) == max(len(old), len(new)),
            "Exact source/target union acquisition required")
    if restore:
        # After a give-way, bank 1 is the source with its first marker byte cleared.
        require(not interrupted and source is not None, "Restore applies to a reviewed source only")
        require(current[:1] == b"\0" and current[1:len(old)] == old[1:],
                "Bank 1 is not the reviewed source with only its first marker byte cleared")
        proposed = new + current[len(new):]
        return proposed, b"\xff" * 8 + proposed[8:]
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
        require(sector in frames, f"Sector {layout.start + at:#x} is neither source, staged target nor erased")
        partial.append(at)
    require(len(partial) <= 1, "More than one partially programmed sector")
    return proposed, staged


def plan(source, target, current, interrupted=False, layout=BANK1, other=None, original=None, restore=False):
    """`other`: the other bank's first sector as inspected; its hash is pinned."""
    proposed, staged = images(source, target, current, interrupted, layout, original, restore)
    changed = current != proposed
    offsets = [i for i in range(0, len(current), SECTOR)
               if current[i:i + SECTOR] != staged[i:i + SECTOR]] if changed else []
    require(interrupted or not changed or offsets[0] == 0, "Marker must be staged first")
    out = {"schema_version": 1, "kind": layout.kind, "bank": layout.bank, "interrupted": interrupted,
            "source_ota_sha256": None if source is None else digest(source), "target_ota_sha256": digest(target),
            "source_payload_bytes": 0 if source is None else len(payload(source, layout)),
            "target_payload_bytes": len(payload(target, layout)),
            "region": {"offset": layout.start, "bytes": len(current)},
            "current_region_sha256": digest(current), "staged_region_sha256": digest(staged),
            "proposed_region_sha256": digest(proposed),
            "ordered_sectors": [{"offset": layout.start + i, "bytes": SECTOR,
                                 "before_sha256": digest(current[i:i + SECTOR]),
                                 "staged_sha256": digest(staged[i:i + SECTOR])} for i in offsets],
            "erase_sectors": len(offsets), "staging_packets": len(offsets) * 4,
            "commit_packets": int(changed), "additional_commit_erases": 0,
            "retained_tail_sha256": digest(current[len(payload(target, layout)):]),
            "scope": "Application union and factory/other-bank guards read back; other flash is not scanned"}
    if other is not None:
        out["other_bank_sha256"] = digest(other)
    if restore:
        out["restore_marker"] = True
    return out


def original_region(full_read, target, layout):
    """A pinned full-flash read's bytes over the target's extent in this bank."""
    require(type(full_read) is bytes and len(full_read) == 0x800000, "A full 8 MiB flash read is required")
    return full_read[layout.start:layout.start + len(payload(target, layout))]


def frame(layout, number, offset, data):
    """A program packet bounded to this layout's bank; never the factory region."""
    require(type(offset) is int and layout.start <= offset and offset + 1024 <= layout.end
            and offset >= FACTORY[1] and offset % 1024 == 0, f"Packet outside aligned bank {layout.bank}")
    if layout.bank == 1:
        return T.program_frame(number, offset, data)
    require(type(number) is int and 1 <= number <= 4, "Packet number must be 1-4")
    require(isinstance(data, bytes) and len(data) == 1024, "Full immutable 1 KiB payload required")
    import struct
    body = struct.pack("<I", 0x08000000 + offset) + data
    return bytes([2, number, number ^ 255]) + body + bytes([sum(body) & 255])


def run(port, baseline, source, target, current, reviewed, acquire, output, progress=None, interrupted=False,
        layout=BANK1, other=None, restore=False, original=None):
    """acquire returns (application union, factory/other-bank guard byte arrays).
    `original`: bank 0's pinned bytes when overwriting it without a source."""
    expected = plan(source, target, current, interrupted, layout, other, original, restore)
    START = layout.start
    other_sha256 = expected.get("other_bank_sha256")
    require(canonical(reviewed) == canonical(expected), "Reviewed development plan differs")
    require(callable(acquire), "Region acquisition callback required")
    proposed, staged = images(source, target, current, interrupted, layout, original, restore)
    # Freeze all packets before creating durable intent or talking to transport.
    sectors = []
    for entry in expected["ordered_sectors"]:
        offset = entry["offset"]
        at = offset - START
        data = staged[at:at + SECTOR]
        frames = tuple(frame(layout, n + 1, offset + n * 1024, data[n * 1024:(n + 1) * 1024])
                       for n in range(4))
        sectors.append((offset, data, frames))
    commit = frame(layout, 1, START, proposed[:1024])
    journal = C.Journal(output)

    def record(event, **fields):
        journal.append({"event": event, **fields})

    def check(expected_bytes, phase):
        data, protected = acquire()
        guards(baseline, protected, layout, other_sha256)
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


GIVE_WAY_KIND = "plaf103_bank1_give_way"


def give_way_plan(baseline, fallback, bank0, bank1_sector):
    """Clear bank 1's first marker byte so the factory loader falls back to
    bank 0. Bank 0 must hold exactly the reviewed `fallback` image (marker
    included): a valid marker over a damaged body would leave nothing to boot.
    One program frame changes byte 0 from '8' to 0x00; NOR programming only
    clears bits, so the other 1023 bytes are rewritten unchanged."""
    T.pinned_baseline(baseline)
    want = payload(fallback, BANK0)
    require(type(bank0) is bytes and bank0 == want, "Bank 0 is not exactly the reviewed fallback image")
    require(want[:8] == MARKER, "The fallback image lacks the loader marker")
    require(type(bank1_sector) is bytes and len(bank1_sector) == SECTOR and bank1_sector[:8] == MARKER,
            "Bank 1's marker is not valid; nothing to give way")
    after = b"\0" + bank1_sector[1:]
    return {"schema_version": 1, "kind": GIVE_WAY_KIND, "bank": 1, "offset": START,
            "fallback_ota_sha256": digest(fallback), "bank0_region_sha256": digest(bank0),
            "before_sha256": digest(bank1_sector), "after_sha256": digest(after),
            "program_frame_sha256": digest(T.program_frame(1, START, after[:1024])),
            "scope": "One 1 KiB program frame at bank 1's start; no erase"}


def run_give_way(port, baseline, fallback, bank0, bank1_sector, reviewed, acquire, output):
    """acquire returns (factory, bank 0 region, bank 1 first sector)."""
    expected = give_way_plan(baseline, fallback, bank0, bank1_sector)
    require(canonical(reviewed) == canonical(expected), "Reviewed give-way plan differs")
    after = b"\0" + bank1_sector[1:]
    packet = T.program_frame(1, START, after[:1024])
    journal = C.Journal(output)

    def record(event, **fields):
        journal.append({"event": event, **fields})

    def check(want_bank1, phase):
        factory, region, sector = acquire()
        require(factory == T.pinned_baseline(baseline)[:0x6000], phase + ": factory region differs")
        require(region == bank0, phase + ": bank 0 differs from the reviewed fallback")
        require(sector == want_bank1, phase + ": bank 1's first sector differs")
        record("guards_verified", phase=phase)

    try:
        record("transaction_start", plan=expected)
        check(bank1_sector, "preimage")
        T.check_status(port, START, START + 1024, "before_give_way", record)
        record("give_way_intent", frame_sha256=digest(packet))
        T.program_packets(port, (packet,), record)
        require(T.read_sector(port, START) == after, "Bank 1 first-sector readback differs")
        check(after, "final")
        record("give_way_verified", after_sha256=digest(after))
        report = {"status": "verified_via_supplied_transports", "after_sha256": digest(after),
                  "application_boot_verified": False, "scope": expected["scope"]}
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
