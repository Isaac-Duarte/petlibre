#!/usr/bin/env python3
"""Verify a finalized receive-only FW15 flash journal capture against its build artifact.

A passing capture is one power-on holding exactly three startups: boot 1 creates
the journal, commits two records and leaves a third reserved, then the watchdog
resets; boot 2 reopens it, sees the pending record, refuses a new id, tears the
pending record and the watchdog resets; boot 3 reopens it, sees the same view,
refuses again, wipes the journal and stays up.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('firmware_startup', ROOT / 'tools/audit/audit-firmware-startup.py')
STARTUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STARTUP)
EVIDENCE = STARTUP.EVIDENCE
POLICY = 'fw15-journal-v1'
BOOTS = 3
TICK_HZ = 32768
CREATE, RESERVE, FINISH, OPEN, TORN, WIPE = range(6)
KINDS = ('create', 'reserve', 'finish', 'open', 'torn', 'wipe')
# (kind, id) per boot, split at the view: ops before the view, ops after the refusal.
OPS = (([(CREATE, 0), (RESERVE, 1), (FINISH, 1), (RESERVE, 2), (FINISH, 2), (RESERVE, 3)], []),
       ([(OPEN, 0)], [(TORN, 0)]),
       ([(OPEN, 0)], [(WIPE, 0)]))
ANY_TICKS = (CREATE, OPEN, TORN)  # may complete within one tick
WRITE_TICKS = 26214  # 800 ms: one journal write is an erase, two programs and verify reads, each in a lease
VIEW = (6, 3, 1, 3, 0, 1)  # sequence, high_water, pending, result, outcome, slot
REFUSE = (4, 0)  # id, reserved
END = ((0, 1, 5, 1, 0), (1, 1, 1, 1, 0), (2, 0, 2, 1, 0))  # mode, reset, erases, phase IDLE, no fault
PREIMAGE = (STARTUP.READ_ID, 16)  # JEDEC ID; all 16 sectors of the owner-approved 64 KiB region blank
RESET_SECONDS = (2.0, 6.5)
CLEAN_TAIL_SECONDS = 30.0
DEC, HEX8 = rb'(\d{1,10})', rb'([0-9a-f]{8})'
RECORDS = {  # letter -> fixed record format
    'S': rb'PLJNL1 start mode=([012]) marker=' + HEX8 + rb' watchdog=(pass|fail) installed=([01])\r\n',
    'P': rb'PLJNL1 preimage id=([0-9a-f]{6}) blank_sectors=' + DEC + rb'\r\n',
    'O': rb'PLJNL1 op kind=' + DEC + rb' id=' + DEC + rb' ok=([01]) error=' + DEC + rb' ticks=' + DEC + rb'\r\n',
    'V': (rb'PLJNL1 view sequence=' + DEC + rb' high_water=' + DEC + rb' pending=([01]) result=' + DEC + rb' outcome=' + DEC
          + rb' slot=' + DEC + rb'\r\n'),
    'R': rb'PLJNL1 refuse id=' + DEC + rb' reserved=([01])\r\n',
    'E': (rb'PLJNL1 end mode=' + DEC + rb' reset=([01]) leases=' + DEC + rb' frozen=' + DEC + rb' erases=' + DEC + rb' phase=' + DEC
          + rb' fault=' + DEC + rb'\r\n'),
    'K': rb'PLJNL1 skip marker=' + HEX8 + rb'\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
SEQUENCE = re.compile(r'SP?O*V?R?O*E?')
LIMIT = ('UART protocol and timing evidence; host timestamps carry USB latency and receive-chunk granularity. '
         'NOR data retention and power-cut behaviour are not established by this capture.')


def records(rx):
    """Fixed-format journal records; any PLJNL1 text that does not parse is reported, not echoed."""
    found = [(m.lastgroup, m, re.fullmatch(RECORDS[m.lastgroup], m[0]).groups()) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLJNL1')


def segments(rx, starts):
    """Split before each later startup, after the last PLJNL1 line that precedes it."""
    bounds, begin = [0], 0
    for start in starts[1:]:
        tail = [begin + m.end() for m in re.finditer(rb'PLJNL1 [^\r\n]*\r\n', rx[begin:start])]
        begin = tail[-1] if tail else start
        bounds.append(begin)
    return bounds, bounds[1:] + [len(rx)]


def numbers(fields):
    return tuple(int(v) for v in fields)


def check_journal(boots, arrival, end_utc, result, first_after):
    """Return the journal outcome for the three parsed boots, filling fixed report fields.

    A reset is timed from the end line's arrival to the first receive chunk that
    starts at or after the next boot's split point."""
    parsed = []
    for base, segment in boots:
        found, _ = records(segment)
        if not SEQUENCE.fullmatch(''.join(k for k, _, _ in found)):
            return 'journal_sequence'
        parsed.append([(kind, base + m.start(), base + m.end(), fields) for kind, m, fields in found])

    def single(found, kind):
        return next((r for r in found if r[0] == kind), None)
    for number, found in enumerate(parsed):
        mode, marker, watchdog, installed = found[0][3]
        if (int(mode), int(marker, 16), watchdog, installed) != (number, number, b'pass', b'1'):
            return 'journal_start'
    for number, found in enumerate(parsed):
        preimage = single(found, 'P')
        if (number == 0) != (preimage is not None) or preimage and (preimage[3][0].decode(), int(preimage[3][1])) != PREIMAGE:
            return 'journal_preimage'
    ops = []
    for found in parsed:
        split = next((i for i, r in enumerate(found) if r[0] in 'VRE'), len(found))
        ops.append(([numbers(r[3]) for r in found[:split] if r[0] == 'O'], [numbers(r[3]) for r in found[split:] if r[0] == 'O']))
    result['boot_ops'] = [[dict(zip(('kind', 'id', 'ok', 'error', 'ticks'), op)) for op in before + after] for before, after in ops]
    writes = [op[4] for before, after in ops for op in before + after if op[0] in (RESERVE, FINISH)]
    if writes:
        result['journal_write_ms_max'] = round(max(writes) * 1000 / TICK_HZ, 1)
    for (before, after), expected in zip(ops, OPS):
        if [[op[:2] for op in part] for part in (before, after)] != [list(part) for part in expected]:
            return 'journal_ops'
        for kind, _, ok, error, ticks in before + after:
            if (ok, error) != (1, 0) or (kind not in ANY_TICKS and ticks <= 0) or (kind in (RESERVE, FINISH) and ticks > WRITE_TICKS):
                return 'journal_ops'
    views = [single(found, 'V') for found in parsed]
    result['boot_views'] = [dict(zip(('sequence', 'high_water', 'pending', 'result', 'outcome', 'slot'), numbers(v[3]))) if v else None
                            for v in views]
    if any(v is None or numbers(v[3]) != VIEW for v in views):
        return 'journal_view'
    refusals = [single(found, 'R') for found in parsed]
    if refusals[0] is not None or any(r is None or numbers(r[3]) != REFUSE for r in refusals[1:]):
        return 'journal_refuse'
    ends = [single(found, 'E') for found in parsed]
    result['boot_ends'] = [dict(zip(('mode', 'reset', 'leases', 'frozen', 'erases', 'phase', 'fault'), numbers(e[3]))) if e else None
                           for e in ends]
    for end, expected in zip(ends, END):
        if end is None:
            return 'journal_end'
        mode, reset, leases, frozen, erases, phase, fault = numbers(end[3])
        if (mode, reset, erases, phase, fault) != expected or leases <= 0 or frozen != leases:
            return 'journal_end'
    result['journal_reset_seconds'] = [(first_after(boots[n + 1][0]) - arrival(ends[n][2] - 1)).total_seconds() for n in (0, 1)]
    if not all(RESET_SECONDS[0] <= s <= RESET_SECONDS[1] for s in result['journal_reset_seconds']):
        return 'journal_reset_timing'
    finished = ends[2][2]
    result['journal_tail_seconds'] = (end_utc - arrival(finished - 1)).total_seconds()
    base, segment = boots[2]
    rest = segment[finished - base:]
    if b'PLJNL1' in rest or b'PLFLS1' in rest or result['journal_tail_seconds'] < CLEAN_TAIL_SECONDS:
        return 'journal_tail'
    counts, words = result['boot_hello_counts'], result['boot_peer_session_high_words']
    if counts != [1, 2, 3] or None in words or words[0] < 1 or any(b <= a for a, b in zip(words, words[1:])):
        return 'journal_sessions'
    return 'pass'


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_flash_handshake_policy') == POLICY, 'Journal policy required')
    require(STARTUP.JOURNAL_MARK in image, 'Journal records absent from artifact')
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    def first_after(offset):
        return EVIDENCE.timestamp(next(utc for start, _, utc in chunks if start >= offset))

    starts = [m.start() for m in re.finditer(rb'PLFW1 start ', rx)]
    result = {'outcome': 'pass', 'startups': len(starts), 'boot_reset_flags': None, 'boot_hello_counts': None,
              'boot_peer_session_high_words': None, 'boot_ops': None, 'boot_views': None, 'boot_ends': None,
              'journal_reset_seconds': None, 'journal_write_ms_max': None, 'journal_tail_seconds': None}
    found, intact = records(rx)
    if not intact or any(kind == 'K' for kind, _, _ in found) or b'PLFLS1 error ' in rx:
        result['outcome'] = 'journal_sequence'
    elif len(starts) != BOOTS:
        result['outcome'] = 'wrong_boot_count'
    if result['outcome'] != 'pass':
        return finish(result, identity, image, rx, tx)
    begins, ends = segments(rx, starts)
    boots = [(begin, rx[begin:end]) for begin, end in zip(begins, ends)]
    for number, (base, segment) in enumerate(boots, 1):
        check = STARTUP.startup(segment, manifest, image, identity, lambda offset, base=base: arrival(base + offset))
        check.pop('start_offset', None)
        result[f'boot{number}'] = check
        if check['outcome'] != 'pass' and result['outcome'] == 'pass':
            result['outcome'] = f"boot{number}_{check['outcome']}"
    checks = [result[f'boot{n}'] for n in range(1, BOOTS + 1)]
    result['boot_reset_flags'] = [c.get('reset_flags') for c in checks]
    result['boot_hello_counts'] = [c.get('flash_boot_count') for c in checks]
    result['boot_peer_session_high_words'] = [int(c['flash_peer_session'], 16) >> 32 if c.get('flash_peer_session') else None for c in checks]
    if result['outcome'] == 'pass':
        result['outcome'] = check_journal(boots, arrival, EVIDENCE.timestamp(metadata['end_utc']), result, first_after)
    return finish(result, identity, image, rx, tx)


def finish(result, identity, image, rx, tx):
    return {**result, 'build_id': identity, 'ota_sha256': STARTUP.digest(image), 'rx_sha256': STARTUP.digest(rx),
            'tx_bytes': len(tx), 'limit': LIMIT}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('capture', 'manifest', 'image', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    if not args.output.resolve().is_relative_to((ROOT / 'private').resolve()):
        parser.error('Evidence output must stay under private/')
    report = verify(args.capture, args.manifest, args.image)
    with args.output.open('x') as stream:
        stream.write(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return 0 if report['outcome'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
