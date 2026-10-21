#!/usr/bin/env python3
"""Verify a finalized receive-only FW06 park round-trip capture against its build artifact.

A passing capture is one power-on holding exactly three startups: boot 1 parks and
releases 100 leases, then stops acknowledging an injected park so the watchdog
resets; boot 2 repeats the batches, then stops releasing an injected park so KM0
times out and the watchdog resets; boot 3 completes the batches and stays up.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_startup', ROOT / 'tools/audit-firmware-startup.py')
STARTUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STARTUP)
EVIDENCE = STARTUP.EVIDENCE
POLICY = 'fw06-park-v1'
BOOTS = 3
LEASES = (25, 50, 75, 100)
EXCHANGE_TICKS = 1310  # 40 ms at 32 768 Hz
INJECT_LEASE = 101
TIMEOUT_PEER = '504b0081'  # KM0 published TIMEOUT after its park budget
RELEASE_TIMEOUT_MS = (650, 760)  # 700 ms KM0 park budget
RESET_SECONDS = (2.0, 6.0)
CLEAN_TAIL_SECONDS = 20.0
DEC, HEX8 = rb'(\d{1,10})', rb'([0-9a-f]{8})'
RECORDS = {  # letter -> fixed record format
    'S': rb'PLPRK1 start mode=([012]) marker=' + HEX8 + rb' watchdog=(pass|fail)\r\n',
    'B': rb'PLPRK1 batch leases=' + DEC + rb' park_max=' + DEC + rb' release_max=' + DEC + rb' frozen=' + DEC + rb' alive=' + DEC + rb'\r\n',
    'I': rb'PLPRK1 inject mode=([01]) lease=' + DEC + rb' begin=([01]) peer=' + HEX8 + rb' ms=' + DEC + rb'\r\n',
    'D': rb'PLPRK1 done leases=' + DEC + rb' marker=' + HEX8 + rb'\r\n',
    'E': rb'PLPRK1 error lease=' + DEC + rb' stage=' + DEC + rb' reply=' + DEC + rb' phase=' + DEC + rb' fault=' + DEC + rb'\r\n',
    'N': rb'PLPRK1 error no_reset ms=' + DEC + rb'\r\n',
    'K': rb'PLPRK1 skip marker=' + HEX8 + rb'\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
SEQUENCE = re.compile(r'SB*(?:I*|D*)')
LIMIT = ('UART timing evidence; host timestamps carry USB latency and receive-chunk granularity. '
         'The physical reset cause and flash exclusion during parks are not established.')


def records(rx):
    """Fixed-format park records; any PLPRK1 text that does not parse is reported, not echoed."""
    found = [(m.lastgroup, m, re.fullmatch(RECORDS[m.lastgroup], m[0]).groups()) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLPRK1')


def segments(rx, starts):
    """Split before each later startup, after the last PLPRK1 line that precedes it."""
    bounds, begin = [0], 0
    for start in starts[1:]:
        tail = [begin + m.end() for m in re.finditer(rb'PLPRK1 [^\r\n]*\r\n', rx[begin:start])]
        begin = tail[-1] if tail else start
        bounds.append(begin)
    return bounds, bounds[1:] + [len(rx)]


def check_park(boots, arrival, end_utc, result, first_after=None):
    """Return the park outcome for the three parsed boots, filling fixed report fields.

    A reset is timed to the first receive chunk that starts at or after the next
    boot's split point: the split follows the old boot's last line, so the chunk
    holding that line can also hold its trailing bytes."""
    first_after = first_after or arrival
    parsed = []
    for base, segment in boots:
        found, _ = records(segment)
        if not SEQUENCE.fullmatch(''.join(k for k, _, _ in found)):
            return 'park_sequence'
        parsed.append([(kind, base + m.start(), base + m.end(), fields) for kind, m, fields in found])
    for number, found in enumerate(parsed):
        mode, marker, watchdog = found[0][3]
        if (int(mode), int(marker, 16), watchdog) != (number, number, b'pass'):
            return 'park_start'
    batches = [[tuple(int(v) for v in fields) for kind, _, _, fields in found if kind == 'B'] for found in parsed]
    for name, index in (('park_max', 1), ('release_max', 2), ('frozen', 3), ('alive', 4)):
        result['boot_' + name] = [[b[index] for b in boot] for boot in batches]
    for boot in batches:
        if tuple(b[0] for b in boot) != LEASES:
            return 'park_batches'
        for leases, park, release, frozen, alive in boot:
            if frozen != leases or alive < leases - 1 or not 0 < park <= EXCHANGE_TICKS or not 0 < release <= EXCHANGE_TICKS:
                return 'park_batches'
    tails = [[(start, end, fields) for kind, start, end, fields in found if kind in 'ID'] for found in parsed]
    kinds = [''.join(kind for kind, _, _, _ in found if kind in 'ID') for found in parsed]

    def inject(fields):
        mode, lease, begin, peer, ms = fields
        return int(mode), int(lease), int(begin), peer.decode(), int(ms)
    for number in (0, 1):
        if not kinds[number].startswith('I'):
            return 'park_inject_ack'
        mode, lease, begin, _, ms = inject(tails[number][0][2])
        if (mode, lease, begin, ms) != (number, INJECT_LEASE, number, 0) or (number == 0 and kinds[0] != 'I'):
            return 'park_inject_ack'
    if kinds[1] != 'II':
        return 'park_inject_release'
    mode, lease, begin, peer, ms = inject(tails[1][1][2])
    result['inject_release_timeout_ms'] = ms
    if (mode, lease, begin, peer) != (1, INJECT_LEASE, 1, TIMEOUT_PEER) or not RELEASE_TIMEOUT_MS[0] <= ms <= RELEASE_TIMEOUT_MS[1]:
        return 'park_inject_release'
    for number, key in ((0, 'inject_ack_reset_seconds'), (1, 'inject_release_reset_seconds')):
        result[key] = (first_after(boots[number + 1][0]) - arrival(tails[number][0][1] - 1)).total_seconds()
    if not all(RESET_SECONDS[0] <= result[key] <= RESET_SECONDS[1] for key in ('inject_ack_reset_seconds', 'inject_release_reset_seconds')):
        return 'park_reset_timing'
    if kinds[2] != 'D':
        return 'park_done'
    _, done_end, (leases, marker) = tails[2][0]
    result['clean_tail_seconds'] = (end_utc - arrival(done_end - 1)).total_seconds()
    base, segment = boots[2]
    if ((int(leases), int(marker, 16)) != (LEASES[-1], 2) or b'PLFLS1' in segment[done_end - base:]
            or result['clean_tail_seconds'] < CLEAN_TAIL_SECONDS):
        return 'park_done'
    counts, words = result['boot_hello_counts'], result['boot_peer_session_high_words']
    if counts != [1, 2, 3] or None in words or words[0] < 1 or any(b <= a for a, b in zip(words, words[1:])):
        return 'park_sessions'
    return 'pass'


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_flash_handshake_policy') == POLICY, 'Park policy required')
    require(STARTUP.PARK_MARK in image, 'Park records absent from artifact')
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    def first_after(offset):
        return EVIDENCE.timestamp(next(utc for start, _, utc in chunks if start >= offset))

    starts = [m.start() for m in re.finditer(rb'PLFW1 start ', rx)]
    result = {'outcome': 'pass', 'startups': len(starts), 'boot_reset_flags': None, 'boot_hello_counts': None,
              'boot_peer_session_high_words': None, 'boot_park_max': None, 'boot_release_max': None,
              'boot_frozen': None, 'boot_alive': None, 'inject_ack_reset_seconds': None,
              'inject_release_timeout_ms': None, 'inject_release_reset_seconds': None, 'clean_tail_seconds': None}
    found, intact = records(rx)
    kinds = {kind for kind, _, _ in found}
    if 'N' in kinds:
        result['outcome'] = 'park_reset_timing'
    elif not intact or kinds & {'E', 'K'}:
        result['outcome'] = 'park_sequence'
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
        result['outcome'] = check_park(boots, arrival, EVIDENCE.timestamp(metadata['end_utc']), result, first_after)
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
