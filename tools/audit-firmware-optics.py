#!/usr/bin/env python3
"""Verify a finalized receive-only FW13 optics capture against its build artifact.

A passing capture is one startup whose LED probe completes, then
`PLOPT1 start created=1` and the probe's records in their fixed formats:
  - `open` with board, ready (fresh flash authority bound to the KM0 peer),
    attached, events and inputs all 1;
  - every planned window in order: a 2 s baseline with both emitters off,
    then six cycles of PA15 alone (2 s), PB3 alone (2 s) and both (4 s), each
    followed by 4 s off. Each window starts where the last ended, lasts at
    least its plan and at most 150 ms more, keeps every poll gap at or below
    the sensor filter's 100 ms observation limit, reads motor STOP and exactly
    the selected emitters on every sample (`stopped=1`), and has its
    `inputs` record and kept edges;
  - a `stall` whose cut came one owner lease (300 ms) to 340 ms after the last
    renewal, with the supervisor's OWNER fault (7), emitters off, motor STOP
    and the supervisor closed, then `done`.
Stage b is unattended and also needs the same inputs lit (high on more than
half the samples) by each emitter selection in every cycle. Stage c needs the operator's blocking seen
through the stock-derived filter in both-emitter windows: food reported both
present and empty, and the outlet both clear and blocked.

Pairing, settling (time to first change after a switch) and the shortest
LOW runs are reported, not judged: they are the measurements.
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
POLICY = 'fw13-optics-v1'
DEC = rb'(\d{1,10})'
T = rb'(\d{1,10}|-)'
INPUTS = ('food', 'outlet', 'feedback')
RECORDS = {
    'start': rb'PLOPT1 start created=([01])\r\n',
    'open': rb'PLOPT1 open board=([01]) ready=([01]) attached=([01]) events=([01]) inputs=([01]) ms=' + DEC + rb'\r\n',
    'timeout': rb'PLOPT1 timeout ms=' + DEC + rb'\r\n',
    'window': (rb'PLOPT1 window n=' + DEC + rb' emitters=(off|a|b|both) start=' + DEC + rb' len=' + DEC
               + rb' samples=' + DEC + rb' gap=' + DEC + rb' edges=' + DEC + rb' dropped=' + DEC
               + rb' food=(unknown|present|empty) outlet=(unknown|clear|blocked) stopped=([01])\r\n'),
    'inputs': (rb'PLOPT1 inputs n=' + DEC + rb' highs=' + DEC + b',' + DEC + b',' + DEC
               + rb' begin=([01]{3}) end=([01]{3}) first=' + T + b',' + T + b',' + T
               + rb' low_min=' + T + b',' + T + b',' + T + rb'\r\n'),
    'edge': rb'PLOPT1 edge n=' + DEC + rb' input=(food|outlet|feedback) level=([01]) ms=' + DEC + rb'\r\n',
    'stall': rb'PLOPT1 stall cut=(\d{1,10}|none) fault=' + DEC + rb' off=([01]) stopped=([01]) closed=([01])\r\n',
    'done': rb'PLOPT1 done windows=' + DEC + rb'\r\n',
    'error': rb'PLOPT1 error kind=(supervisor|clock|capture|observe|optics) n=' + DEC + rb' fault=' + DEC + rb'\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
BASELINE_MS, OFF_MS, CYCLES = 2000, 4000, 6
CYCLE = (('a', 2000), ('b', 2000), ('both', 4000))
WINDOWS = 1 + CYCLES * 2 * len(CYCLE)
EDGE_CAP, LATE_MS, MAX_GAP_MS = 16, 150, 100
LEASE_MS, CUT_MAX_MS, OWNER_FAULT = 300, 340, 7
LIMIT = ('UART evidence of GPIO logic levels polled about every 2 ms: settling and pulse lengths carry that '
         'resolution, and receiver levels do not establish beam geometry, emitter current or thermal limits.')


def plan(n):
    if n == 0:
        return 'off', BASELINE_MS
    i = (n - 1) % (2 * len(CYCLE))
    return ('off', OFF_MS) if i % 2 else CYCLE[i // 2]


def records(rx):
    found = [(m.lastgroup, re.fullmatch(RECORDS[m.lastgroup], m[0]).groups(), m.start()) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLOPT1')


def time(value):
    return None if value == b'-' else int(value)


def parse_window(f):
    return {'n': int(f[0]), 'emitters': f[1].decode(), 'start': int(f[2]), 'len': int(f[3]), 'samples': int(f[4]),
            'gap': int(f[5]), 'edges': int(f[6]), 'dropped': int(f[7]), 'food': f[8].decode(),
            'outlet': f[9].decode(), 'stopped': f[10] == b'1'}


def parse_inputs(f):
    return {'n': int(f[0]), 'highs': [int(v) for v in f[1:4]], 'begin': [c == ord('1') for c in f[4]],
            'end': [c == ord('1') for c in f[5]], 'first': [time(v) for v in f[6:9]],
            'low_min': [time(v) for v in f[9:12]]}


def check(rx, found, stage, result):
    if not found or found[0][0] != 'start' or any(kind == 'start' for kind, _, _ in found[1:]):
        return 'optics_sequence'
    led_done = rx.find(b'PLLED1 done low=all\r\n')
    if led_done < 0 or found[0][2] < led_done:
        return 'optics_sequence'
    if found[0][1] != (b'1',):
        return 'optics_start'
    rest = found[1:]
    if any(kind == 'error' for kind, _, _ in rest):
        kind, fields, _ = next(r for r in rest if r[0] == 'error')
        result['optics_error'] = {'kind': fields[0].decode(), 'n': int(fields[1]), 'fault': int(fields[2])}
        return 'optics_error'
    if not rest or rest[0][0] == 'timeout':
        return 'optics_timeout'
    if rest[0][0] != 'open' or rest[0][1][:5] != (b'1',) * 5:
        return 'optics_open'
    result['optics_open_ms'] = int(rest[0][1][5])
    windows, inputs, edges, stall, done = [], {}, {}, None, None
    for kind, f, _ in rest[1:]:
        if done is not None:
            return 'optics_sequence'
        if kind == 'window':
            # A window's records come before the next window's.
            if stall or (windows and windows[-1]['n'] not in inputs):
                return 'optics_sequence'
            windows.append(parse_window(f))
        elif kind == 'inputs':
            entry = parse_inputs(f)
            if not windows or windows[-1]['n'] != entry['n'] or entry['n'] in inputs:
                return 'optics_sequence'
            inputs[entry['n']] = entry
        elif kind == 'edge':
            n = int(f[0])
            if not windows or windows[-1]['n'] != n or n not in inputs:
                return 'optics_sequence'
            edges.setdefault(n, []).append({'input': f[1].decode(), 'level': f[2] == b'1', 'ms': int(f[3])})
        elif kind == 'stall':
            if stall:
                return 'optics_sequence'
            stall = {'cut_ms': None if f[0] == b'none' else int(f[0]), 'fault': int(f[1]), 'off': f[2] == b'1',
                     'stopped': f[3] == b'1', 'closed': f[4] == b'1'}
        elif kind == 'done':
            if stall is None:
                return 'optics_sequence'
            done = int(f[0])
        else:
            return 'optics_sequence'
    result['optics_windows'] = len(windows)
    if len(windows) != WINDOWS or [w['n'] for w in windows] != list(range(WINDOWS)):
        return 'optics_incomplete'
    previous_end = None
    for w in windows:
        emitters, length = plan(w['n'])
        i = inputs.get(w['n'])
        if w['emitters'] != emitters or i is None:
            return 'optics_sequence'
        if previous_end is not None and w['start'] != previous_end:
            return 'optics_sequence'
        previous_end = w['start'] + w['len']
        if not length <= w['len'] <= length + LATE_MS:
            return 'optics_window'
        if w['gap'] > MAX_GAP_MS or w['samples'] == 0 or any(h > w['samples'] for h in i['highs']):
            return 'optics_gap'
        kept = edges.get(w['n'], [])
        if len(kept) != min(w['edges'], EDGE_CAP) or w['dropped'] != w['edges'] - len(kept):
            return 'optics_sequence'
        if not w['stopped']:
            return 'optics_stopped'
    if stall is None or done is None:
        return 'optics_incomplete'
    result['optics_stall'] = stall
    if done != WINDOWS:
        return 'optics_sequence'
    if (stall['cut_ms'] is None or not LEASE_MS <= stall['cut_ms'] <= CUT_MAX_MS or stall['fault'] != OWNER_FAULT
            or not (stall['off'] and stall['stopped'] and stall['closed'])):
        return 'optics_stall'

    def lit(w):
        i = inputs[w['n']]
        return [2 * h > w['samples'] for h in i['highs']]

    pairing = {}
    for w in windows:
        pairing.setdefault(w['emitters'], set()).add(tuple(lit(w)))
    result['optics_pairing'] = {e: [list(INPUTS[k] for k, on in enumerate(s) if on) for s in sorted(v)]
                                for e, v in pairing.items()}
    settle = {}
    for w in windows[1:]:
        for k, ms in enumerate(inputs[w['n']]['first']):
            if ms is not None:
                settle.setdefault(f"{w['emitters']}:{INPUTS[k]}", []).append(ms)
    result['optics_first_change_ms'] = {k: {'min': min(v), 'max': max(v), 'count': len(v)} for k, v in sorted(settle.items())}
    low = {}
    for w in windows:
        for k, ms in enumerate(inputs[w['n']]['low_min']):
            if ms is not None:
                low.setdefault(INPUTS[k], []).append(ms)
    result['optics_low_min_ms'] = {k: min(v) for k, v in sorted(low.items())}
    both = [w for w in windows if w['emitters'] == 'both']
    result['optics_both_states'] = [{'n': w['n'], 'food': w['food'], 'outlet': w['outlet']} for w in both]
    result['optics_max_gap_ms'] = max(w['gap'] for w in windows)
    result['optics_edges'] = sum(w['edges'] for w in windows)
    # The operator's blocking changes what is lit, so consistency is a stage b check.
    if stage == 'b' and any(len(v) != 1 for v in pairing.values()):
        return 'optics_inconsistent'
    if stage == 'c':
        foods, outlets = {w['food'] for w in both}, {w['outlet'] for w in both}
        if not ({'present', 'empty'} <= foods and {'clear', 'blocked'} <= outlets):
            return 'optics_unblocked'
    return 'pass'


def verify(capture, manifest_path, image_path, stage):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_optics_policy') == POLICY, 'Optics policy required')
    require(stage in ('b', 'c'), 'Stage b or c required')
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = {'outcome': 'pass', 'stage': stage, 'startups': len(re.findall(rb'PLFW1 start ', rx))}
    check_startup = STARTUP.startup(rx, manifest, image, identity, arrival, EVIDENCE.timestamp(metadata['end_utc']))
    check_startup.pop('start_offset', None)
    result['startup'] = check_startup
    found, intact = records(rx)
    if check_startup['outcome'] != 'pass':
        result['outcome'] = f"startup_{check_startup['outcome']}"
    elif not intact:
        result['outcome'] = 'optics_sequence'
    else:
        result['outcome'] = check(rx, found, stage, result)
    return {**result, 'policy': POLICY, 'build_id': identity, 'ota_sha256': STARTUP.digest(image),
            'rx_sha256': STARTUP.digest(rx), 'tx_bytes': len(tx), 'limit': LIMIT}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('capture', 'manifest', 'image', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--stage', choices=('b', 'c'), required=True)
    args = parser.parse_args()
    if not args.output.resolve().is_relative_to((ROOT / 'private').resolve()):
        parser.error('Evidence output must stay under private/')
    report = verify(args.capture, args.manifest, args.image, args.stage)
    with args.output.open('x') as stream:
        stream.write(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return 0 if report['outcome'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
