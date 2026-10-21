#!/usr/bin/env python3
"""Verify a finalized receive-only FW05 supervision capture against its build artifact.

A passing capture holds exactly two startups: a cold boot that runs the timer and
watchdog probe until one deliberate watchdog reset, then a boot that skips it.
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
POLICY = 'fw05-probe-v1'
HZ, PERIOD_TICKS, WRAP = 32768, 328, 1 << 32
WATCHDOG_COUNTS = 4095 * 32  # count id 11, divisor 31
MARKER = 0x50465735
VENDOR_FIELDS, VENDOR_EXPECTED = 0x1e00ffff, (11 << 25) | 31
ENABLE, RST_MODE, BIT31 = 1 << 16, 1 << 30, 1 << 31
FAULT_PROGRESS = 4
SNAP = rb' raw=([0-9a-f]{8}) ms=(\d{1,10}) irqs=(\d{1,10}) ready=([01]) healthy=([01]) stopped=([01])\r\n'
RECORDS = {  # letter -> fixed record format, in probe order
    'O': rb'PLTMR1 open board=([01]) events=([01]) hz=32768 period_ms=10 status=(pass|fail)\r\n',
    'S': rb'PLTMR1 sample' + SNAP,
    'B': rb'PLTMR1 stall_begin' + SNAP,
    'E': rb'PLTMR1 stall_end' + SNAP,
    'A': rb'PLWDG1 arm status=(pass|fail) vendor=([0-9a-f]{8}) count=(\d{1,10}) divisor=(\d{1,10}) marker=([0-9a-f]{8})\r\n',
    'F': rb'PLWDG1 fed ms=(\d{1,10}) checkpoints=(\d{1,10}) status=(pass|fail)\r\n',
    'W': rb'PLWDG1 stall_begin ms=(\d{1,10})\r\n',
    'T': rb'PLWDG1 stalled ms=(\d{1,10}) fault=(\d{1,10}) enabled=([01])\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
SKIP = re.compile(rb'PLWDG1 skip reset=([0-9a-f]{8}) marker=([0-9a-f]{8})\r\n')
SEQUENCE = re.compile(r'O(?:S*(?:B(?:E(?:A(?:F(?:WT*)?)?)?)?)?)?')
LED_DONE = b'PLLED1 done low=all\r\n'
LIMIT = ('UART timing evidence; host timestamps carry USB latency and receive-chunk granularity. '
         'Electrical outputs and the physical reset cause are not established.')


def records(rx):
    """Fixed-format probe records; any PLTMR1/PLWDG1 text that does not parse is reported, not echoed."""
    found = []
    for m in ANY.finditer(rx):
        kind = m.lastgroup
        fields = re.fullmatch(RECORDS[kind], m[0]).groups()
        found.append((kind, m, fields))
    intact = len(found) == rx.count(b'PLTMR1') + rx.count(b'PLWDG1')
    return found, intact


def snapshot(fields):
    raw, ms, irqs = int(fields[0], 16), int(fields[1]), int(fields[2])
    return {'raw': raw, 'ms': ms, 'irqs': irqs, 'ok': fields[3:] == (b'1', b'1', b'1')}


def delta(a, b):
    return (b - a) % WRAP


def within(actual, expected, fraction):
    return abs(actual - expected) <= max(2, fraction * expected)


def check_probe(boot1, arrival):
    """Return (outcome, report fields) for boot 1's timer, stall, and watchdog records."""
    report = {}
    found, intact = records(boot1)
    kinds = ''.join(k for k, _, _ in found)
    led = boot1.find(LED_DONE)
    if not intact or not SEQUENCE.fullmatch(kinds) or led < 0 or found[0][1].start() < led + len(LED_DONE):
        return 'supervision_sequence', report
    by = {}
    for kind, m, fields in found:
        by.setdefault(kind, []).append((m, fields))
    board, events, status = by['O'][0][1]
    if (board, events, status) != (b'1', b'1', b'pass'):
        return 'timer_open', report
    samples = [(m, snapshot(f)) for m, f in by.get('S', [])]
    report['timer_samples'] = len(samples)
    if len(samples) != 13 or not all(s['ok'] for _, s in samples):
        return 'timer_samples', report
    for (_, a), (_, b) in zip(samples, samples[1:]):
        ticks = delta(a['raw'], b['raw'])
        if not 0 < ticks < WRAP // 2:
            return 'timer_consistency', report
        if abs(delta(a['ms'], b['ms']) - ticks * 1000 / HZ) > 2 or not within(delta(a['irqs'], b['irqs']), ticks / PERIOD_TICKS, 0.01):
            return 'timer_consistency', report
    ticks = sum(delta(a['raw'], b['raw']) for (_, a), (_, b) in zip(samples, samples[1:]))
    irqs = sum(delta(a['irqs'], b['irqs']) for (_, a), (_, b) in zip(samples, samples[1:]))
    seconds = (arrival(samples[-1][0].end() - 1) - arrival(samples[0][0].end() - 1)).total_seconds()
    report['sample_host_seconds'] = seconds
    if seconds < 50:
        return 'timer_samples', report
    hz = ticks / seconds
    report.update(systimer_hz_estimate=round(hz, 1), systimer_error_ppm=round((hz / HZ - 1) * 1e6),
                  tim2_period_ms_estimate=round(seconds * 1000 / irqs, 4) if irqs else None)
    if abs(hz / HZ - 1) > 0.02:
        return 'systimer_rate', report
    if 'E' not in by:
        return 'stall_failed', report
    begin, end = snapshot(by['B'][0][1]), snapshot(by['E'][0][1])
    ticks = delta(begin['raw'], end['raw'])
    report['stall_ticks'] = ticks
    if not 65536 <= ticks < WRAP // 2 or not within(delta(begin['irqs'], end['irqs']), ticks / PERIOD_TICKS, 0.02) or not end['ok']:
        return 'stall_failed', report
    if 'A' not in by:
        return 'watchdog_arm', report
    status, vendor, count, divisor, marker = by['A'][0][1]
    vendor, marker = int(vendor, 16), int(marker, 16)
    report.update(watchdog_vendor=f'{vendor:08x}', watchdog_marker=f'{marker:08x}')
    if (status != b'pass' or (int(count), int(divisor)) != (11, 31) or marker != MARKER or vendor & VENDOR_FIELDS != VENDOR_EXPECTED
            or vendor & (ENABLE | RST_MODE) != ENABLE | RST_MODE or vendor & BIT31):
        return 'watchdog_arm', report
    if 'F' not in by:
        return 'watchdog_feed', report
    ms, checkpoints, status = by['F'][0][1]
    report.update(watchdog_fed_ms=int(ms), watchdog_checkpoints=int(checkpoints))
    if status != b'pass' or int(ms) < 15000 or int(checkpoints) < 50:
        return 'watchdog_feed', report
    if 'W' not in by:
        return 'watchdog_stall', report
    stalled = [tuple(int(v) for v in f) for _, f in by.get('T', [])]
    report['watchdog_stalled_reports'] = len(stalled)
    if any((fault, enabled) != (FAULT_PROGRESS, 1) for _, fault, enabled in stalled):
        return 'watchdog_stall', report
    report['last_stalled_ms'] = stalled[-1][0] if stalled else None
    report['stall_line_end'] = by['W'][0][0].end()
    return 'pass', report


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_supervision_policy') == POLICY, 'Supervision policy required')
    require(STARTUP.SUPERVISION_MARK in image, 'Supervision records absent from artifact')
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = {'outcome': 'pass'}
    starts = [m.start() for m in re.finditer(rb'PLFW1 start ', rx)]
    result['startups'] = len(starts)
    if b'PLTMR1 error clock\r\n' in rx:
        result['outcome'] = 'timer_clock'
    elif re.search(rb'PLWDG1 error no_reset ms=\d{1,10}\r\n', rx):
        result['outcome'] = 'watchdog_no_reset'
    elif b'PLTMR1 error' in rx or b'PLWDG1 error' in rx:
        result['outcome'] = 'supervision_sequence'
    elif len(starts) != 2:
        result['outcome'] = 'wrong_boot_count'
    if result['outcome'] != 'pass':
        return finish(result, identity, image, rx, tx)
    tail = [m.end() for m in re.finditer(rb'PLWDG1 [^\r\n]*\r\n', rx[:starts[1]])]
    split = tail[-1] if tail else starts[1]
    boot1, boot2 = rx[:split], rx[split:]
    for name, segment in (('boot1', boot1), ('boot2', boot2)):
        check = STARTUP.startup(segment, manifest, image, identity)
        check.pop('start_offset', None)
        result[name] = check
        if check['outcome'] != 'pass' and result['outcome'] == 'pass':
            result['outcome'] = f"{name}_{check['outcome']}"
    result['boot2_reset_flags'] = result['boot2'].get('reset_flags')
    if result['outcome'] != 'pass':
        return finish(result, identity, image, rx, tx)
    outcome, probe = check_probe(boot1, arrival)
    stall_end = probe.pop('stall_line_end', None)
    last_stalled = probe.pop('last_stalled_ms', None)
    result.update(probe)
    if outcome == 'pass':
        interval = (arrival(split) - arrival(stall_end - 1)).total_seconds()
        result.update(watchdog_reset_seconds=interval,
                      watchdog_clock_hz_estimate=round(WATCHDOG_COUNTS / interval, 1) if interval > 0 else None,
                      watchdog_clock_note='Includes reset/boot latency and up to one 250 ms refresh offset.')
        if not 2.5 <= interval <= 7.0 or (last_stalled is not None and last_stalled >= interval * 1000 + 500):
            outcome = 'reset_timing'
    if outcome == 'pass':
        skips = list(SKIP.finditer(boot2))
        found, _ = records(boot2)
        done = boot2.find(LED_DONE)
        result['boot2_skip_reset'] = skips[0][1].decode() if len(skips) == 1 else None
        if (len(skips) != 1 or int(skips[0][2], 16) != MARKER or found or done < 0 or skips[0].start() < done + len(LED_DONE)
                or boot2.count(b'PLTMR1') + boot2.count(b'PLWDG1') != 1):
            outcome = 'boot2_skip'
    result['outcome'] = outcome
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
