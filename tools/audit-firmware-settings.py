#!/usr/bin/env python3
"""Verify a finalized receive-only FW15 settings power-cut capture against its build artifact.

The capture is one receive-only recording during which the operator repeatedly cuts
and restores feeder power while each boot saves settings (up to 300 saves per
boot). Boots are separated by power cuts, so every startup must report reset flags
00000000, and a boot's output may stop anywhere, mid-line included. A passing capture
holds at least three boots that opened the store consistently, each recovering a
generation no older than the previous boot's last report and at most 25 newer, with
at least two boots cut between their open and end records.
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
POLICY = 'fw15-settings-v1'
TICK_HZ = 32768
LOADED, EMPTY, RECOVERED = 0, 1, 2  # also 3 CORRUPT, 4 FUTURE, 5 IO, 6 WORN
OPENED_BOOTS = 3
CUT_BOOTS = 2
BATCH = 25
SAVES = 300  # the end record replaces the batch record at the 300th save
RECOVERY_WINDOW = 25  # up to 24 saves after the last batch plus one interrupted save
SAVE_TICKS = 26214  # 800 ms: one settings save is an erase, a program and verify reads, each in a lease
POWER_OFF_SECONDS = 0.5  # a power cut leaves at least this reception gap before the next boot's output
DEC, HEX8 = rb'(\d{1,20})', rb'([0-9a-f]{8})'
RECORDS = {  # letter -> fixed record format
    'S': rb'PLSET1 start marker=' + HEX8 + rb' watchdog=(pass|fail) installed=([01])\r\n',
    'O': (rb'PLSET1 open status=' + DEC + rb' generation=' + DEC + rb' volume=' + DEC + rb' lights=([01]) slot=' + DEC
          + rb' consistent=([01])\r\n'),
    'B': rb'PLSET1 batch saves=' + DEC + rb' generation=' + DEC + rb' ticks_max=' + DEC + rb'\r\n',
    'E': (rb'PLSET1 end saves=' + DEC + rb' generation=' + DEC + rb' leases=' + DEC + rb' frozen=' + DEC + rb' phase=' + DEC
          + rb' fault=' + DEC + rb'\r\n'),
    'X': rb'PLSET1 error save generation=' + DEC + rb' status=' + DEC + rb'\r\n',
    'K': rb'PLSET1 skip marker=' + HEX8 + rb'\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
SEQUENCE = re.compile(r'(?:S(?:OB*E?)?)?')
FAILURES = (b'PLSET1 error', b'PLSET1 skip', b'PLFLS1 error')
FOREIGN = (b'PLPRK1', b'PLRDS1', b'PLWRS1', b'PLJNL1')
OPEN_FIELDS = ('status', 'generation', 'volume', 'lights', 'slot', 'consistent')
END_FIELDS = ('saves', 'generation', 'leases', 'frozen', 'phase', 'fault')
LIMIT = ('UART protocol evidence; host timestamps carry USB latency and receive-chunk granularity. '
         'The power-cut instants are operator-chosen and not measured, and flash retention beyond the capture is not established.')


def records(segment):
    """Complete fixed-format settings records, and the offset of a final cut fragment.

    Only the last PLSET1 text of a segment may fail to parse, and only when no complete
    line follows it: power was cut while it was printed. Any other unparsed PLSET1 text
    is malformed (None)."""
    found, fragment = [], None
    marks = [m.start() for m in re.finditer(b'PLSET1', segment)]
    for index, at in enumerate(marks):
        m = ANY.match(segment, at)
        if m:
            fields = re.fullmatch(RECORDS[m.lastgroup], m[0]).groups()
            found.append((m.lastgroup, m, fields if m.lastgroup in 'SK' else tuple(int(v) for v in fields)))
        elif index == len(marks) - 1 and b'\r\n' not in segment[at:]:
            fragment = at
        else:
            return None, None
    return found, fragment


def boundaries(starts, chunks):
    """Each later boot begins at the receive chunk after the power-off gap preceding its startup.

    Returns the segment begin offsets, or None when no such gap follows the previous startup."""
    bounds = [0]
    for previous, start in zip(starts, starts[1:]):
        index = next(i for i, (begin, end, _) in enumerate(chunks) if begin <= start < end)
        while True:
            if index == 0 or chunks[index][0] <= previous:
                return None
            if (chunks[index][2] - chunks[index - 1][2]).total_seconds() >= POWER_OFF_SECONDS:
                break
            index -= 1
        bounds.append(chunks[index][0])
    return bounds


def check_settings(opened, result):
    """Return the settings outcome for the opened boots: (boot number, parsed records, cut)."""
    if len(opened) < OPENED_BOOTS:
        return 'settings_boots'
    lasts, deltas, ticks, ends = [], [], [], []
    for _, found, _ in opened:
        lasts.append(max(fields[1] for kind, _, fields in found if kind in 'OBE'))
        ticks += [fields[2] for kind, _, fields in found if kind == 'B']
        ends.append(next((dict(zip(END_FIELDS, fields)) for kind, _, fields in found if kind == 'E'), None))
    opens = [next(fields for kind, _, fields in found if kind == 'O') for _, found, _ in opened]
    result['boot_last_generations'] = [None] * result['startups']
    for (number, _, _), last in zip(opened, lasts):
        result['boot_last_generations'][number - 1] = last
    result['settings_recovery_deltas'] = [o[1] - last for o, last in zip(opens[1:], lasts)]
    if ticks:
        result['settings_ticks_max'] = max(ticks)
        result['settings_save_ms_max'] = round(max(ticks) * 1000 / TICK_HZ, 1)
    result['settings_cuts'] = sum(cut for _, _, cut in opened)
    result['boot_ends'] = ends
    for index, ((_, found, _), fields) in enumerate(zip(opened, opens)):
        status, generation, _, _, _, consistent = fields
        if found[0][2] != (b'00000000', b'pass', b'1') or consistent != 1 or not (
                status in (LOADED, RECOVERED) or (index == 0 and status == EMPTY and generation == 0)):
            return 'settings_open'
    if any(not 0 <= delta <= RECOVERY_WINDOW for delta in result['settings_recovery_deltas']):
        return 'settings_recovery'
    for (_, found, _), (_, generation, *_) in zip(opened, opens):
        batches = [fields for kind, _, fields in found if kind == 'B']
        if [b[0] for b in batches] != list(range(BATCH, BATCH * len(batches) + 1, BATCH)) or len(batches) >= SAVES // BATCH:
            return 'settings_batches'
        if any(b[1] != generation + b[0] or not 0 < b[2] <= SAVE_TICKS for b in batches):
            return 'settings_batches'
    for (_, found, _), (_, generation, *_), end in zip(opened, opens, ends):
        batches = sum(kind == 'B' for kind, _, _ in found)
        if end and ((end['saves'], end['generation'], end['phase'], end['fault']) != (SAVES, generation + SAVES, 1, 0)
                    or end['leases'] <= 0 or end['frozen'] != end['leases'] or batches != SAVES // BATCH - 1):
            return 'settings_end'
    if result['settings_cuts'] < CUT_BOOTS:
        return 'settings_cuts'
    return 'pass'


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_flash_handshake_policy') == POLICY, 'Settings policy required')
    require(STARTUP.SETTINGS_MARK in image, 'Settings records absent from artifact')
    chunks = [(e['offset'], e['offset'] + e['bytes'], EVIDENCE.timestamp(e['utc'])) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return next(utc for start, end, utc in chunks if start <= offset < end)

    starts = [m.start() for m in re.finditer(rb'PLFW1 start ', rx)]
    result = {'outcome': 'pass', 'startups': len(starts), 'boot_reset_flags': None, 'boot_opened': None, 'boot_startups': None,
              'boot_opens': None, 'boot_last_generations': None, 'settings_recovery_deltas': None, 'settings_ticks_max': None,
              'settings_save_ms_max': None, 'settings_cuts': None, 'boot_ends': None}
    bounds = boundaries(starts, chunks) if starts else None
    parsed = []
    if bounds is not None:
        for begin, end in zip(bounds, bounds[1:] + [len(rx)]):
            found, fragment = records(rx[begin:end])
            if found is None or not SEQUENCE.fullmatch(''.join(k for k, _, _ in found)) or (
                    fragment is not None and found and found[-1][0] == 'E'):
                break
            parsed.append((begin, rx[begin:end], found))
    if bounds is None or len(parsed) != len(starts) or any(f in rx for f in FAILURES + FOREIGN):
        result['outcome'] = 'settings_sequence' if starts else 'settings_boots'
        return finish(result, identity, image, rx, tx)
    flags = [STARTUP.START.match(rx, start) for start in starts]
    result['boot_reset_flags'] = [m[3].decode() if m else None for m in flags]
    result['boot_opened'] = [any(kind == 'O' for kind, _, _ in found) for _, _, found in parsed]
    result['boot_opens'] = [next((dict(zip(OPEN_FIELDS, fields)) for kind, _, fields in found if kind == 'O'), None)
                            for _, _, found in parsed]
    if any(flag not in (None, '00000000') for flag in result['boot_reset_flags']):
        result['outcome'] = 'wrong_reset'
        return finish(result, identity, image, rx, tx)
    checks, opened = [], []
    for number, ((base, segment, found), is_open) in enumerate(zip(parsed, result['boot_opened']), 1):
        if not is_open:
            checks.append(None)
            continue
        # The boot completed startup; its output up to its last complete record is audited as one startup.
        check = STARTUP.startup(segment[:found[-1][1].end()], manifest, image, identity, lambda offset, base=base: arrival(base + offset))
        check.pop('start_offset', None)
        checks.append(check)
        if check['outcome'] != 'pass' and result['outcome'] == 'pass':
            result['outcome'] = f"boot{number}_{check['outcome']}"
        cut = number < len(parsed) and found[-1][0] != 'E'
        opened.append((number, found, cut))
    result['boot_startups'] = checks
    outcome = check_settings(opened, result)
    if result['outcome'] == 'pass':
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
