#!/usr/bin/env python3
"""Verify a finalized receive-only FW14 power-input capture against its build artifact.

A passing capture is one startup whose LED probe completes, then the read-only ADC
probe: the explicit LP ADC clock and function enable reads back with no other bit
changed; the efuse calibration bytes decode to the reported offset and gain; and at
least 30 samples follow, numbered from 0, about a second apart, each with stopped
outputs, every value in range, and each scaled value matching the stock formula on
its mean. Under fw14-adc-v2 every sample also carries internal channel 10 (the
chip's own supply), and its mean must differ from both external channels' means
by at least 1000 raw: evidence that the channel select takes effect. The input
values themselves are otherwise observations: their electrical meaning needs a
measurement.
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
POLICIES = ('fw14-adc-v1', 'fw14-adc-v2')  # v2 adds the internal supply channel
DISTINCT_RAW = 1000
ADC_BIT = 1 << 8  # BIT_LSYS_ADC_FEN / BIT_LSYS_ADC_CKE
MIN_SAMPLES = 30
SAMPLE_SECONDS = (0.8, 1.5)  # mean host spacing of samples; firmware waits 1000 ms after each
CAPTURE_TICKS = 3277  # 100 ms at 32 768 Hz for 32 conversions
MAX_RAW = 4095
DEFAULT_OFFSET, DEFAULT_GAIN = 2480, 12050
DEC, HEX8 = rb'(\d{1,10})', rb'([0-9a-f]{8})'
CHANNEL = rb'(\d{1,4}),(\d{1,4}),(\d{1,4}),(\d{1,10})'
RECORDS = {  # letter -> fixed record format
    'C': (rb'PLADC1 clock function_before=' + HEX8 + rb' clock_before=' + HEX8 + rb' function_after=' + HEX8
          + rb' clock_after=' + HEX8 + rb' status=(pass|fail)\r\n'),
    'O': (rb'PLADC1 open outputs=([01]) efuse=([0-9a-f]{8}|none) offset=' + DEC + rb' gain=' + DEC
          + rb' default_offset=([01]) default_gain=([01]) status=(pass|fail)\r\n'),
    'S': (rb'PLADC1 sample n=' + DEC + rb' ticks=' + DEC + rb' food=([01]) outlet=([01]) dc=([01]) stopped=([01]) current='
          + CHANNEL + rb' battery=' + CHANNEL + rb'(?: supply=' + CHANNEL + rb')?\r\n'),
    'E': rb'PLADC1 error n=' + DEC + rb' ticks=' + DEC + rb' stopped=([01])\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
FOREIGN = (b'PLFLS1', b'PLPRK1', b'PLRDS1', b'PLWRS1', b'PLJNL1', b'PLSET1', b'PLTMR1')
LIMIT = ('UART evidence of register readback and raw conversions; host timestamps carry USB latency. Voltages, '
         'divider ratios, current units and the external-power polarity are not established by this capture.')


def records(rx):
    """Fixed-format ADC records; PLADC1 text that does not parse is reported, not echoed."""
    found = [(m.lastgroup, m) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLADC1')


def scaled(mean, offset, gain):
    """Stock order: floor mean in tenths, minus offset, times 1000, over gain; 0 at or below offset."""
    return 0 if mean * 10 <= offset else (mean * 10 - offset) * 1000 // gain


def calibration(efuse):
    """(offset, gain, default_offset, default_gain) for the efuse bytes 0x1d0..0x1d3 in read order."""
    raw = bytes.fromhex(efuse)
    offset, gain = int.from_bytes(raw[:2], 'little'), int.from_bytes(raw[2:], 'little')
    return (DEFAULT_OFFSET if offset == 0xffff else offset, DEFAULT_GAIN if gain == 0xffff else gain,
            int(offset == 0xffff), int(gain == 0xffff))


def check_adc(rx, found, arrival, result, supply=False):
    """Return the ADC outcome for one startup's parsed records, filling fixed report fields."""
    kinds = ''.join(kind for kind, _ in found)
    if not re.fullmatch(r'COS*', kinds):
        return 'adc_sequence'
    led_done = rx.find(b'PLLED1 done low=all\r\n')
    if led_done < 0 or found[0][1].start() < led_done:
        return 'adc_sequence'
    before_f, before_c, after_f, after_c, status = re.fullmatch(RECORDS['C'], found[0][1][0]).groups()
    before_f, before_c, after_f, after_c = (int(v, 16) for v in (before_f, before_c, after_f, after_c))
    result['adc_enabled_before'] = bool(before_f & before_c & ADC_BIT)
    result['adc_clock'] = {'function_before': f'{before_f:08x}', 'clock_before': f'{before_c:08x}',
                           'function_after': f'{after_f:08x}', 'clock_after': f'{after_c:08x}'}
    if status != b'pass' or not after_f & after_c & ADC_BIT or (after_f & ~ADC_BIT, after_c & ~ADC_BIT) != (before_f & ~ADC_BIT, before_c & ~ADC_BIT):
        return 'adc_clock'
    outputs, efuse, offset, gain, default_offset, default_gain, status = re.fullmatch(RECORDS['O'], found[1][1][0]).groups()
    reported = (int(offset), int(gain), int(default_offset), int(default_gain))
    result['adc_calibration'] = dict(zip(('offset', 'gain', 'default_offset', 'default_gain'), reported))
    if outputs != b'1' or status != b'pass' or efuse == b'none' or calibration(efuse.decode()) != reported:
        return 'adc_open'
    samples = [tuple(-1 if v is None else int(v) for v in re.fullmatch(RECORDS['S'], m[0]).groups()) for _, m in found[2:]]
    if any((s[14] >= 0) != supply for s in samples):
        return 'adc_sequence'
    result['adc_samples'] = len(samples)
    if not samples:
        return 'adc_samples'
    result['adc_ticks_max'] = max(s[1] for s in samples)
    result['adc_capture_ms_max'] = round(result['adc_ticks_max'] * 1000 / 32768, 2)
    for name, index in (('food', 2), ('outlet', 3), ('dc', 4)):
        result[f'adc_{name}_levels'] = sorted({s[index] for s in samples})
    channels = (('current', 6), ('battery', 10)) + ((('supply', 14),) if supply else ())
    for name, base in channels:
        values = [s[base:base + 4] for s in samples]
        result[f'adc_{name}'] = {'min': min(v[0] for v in values), 'max': max(v[1] for v in values),
                                 'mean_min': min(v[2] for v in values), 'mean_max': max(v[2] for v in values),
                                 'spread_max': max(v[1] - v[0] for v in values),
                                 'scaled_min': min(v[3] for v in values), 'scaled_max': max(v[3] for v in values)}
    for number, s in enumerate(samples):
        if s[0] != number or s[5] != 1 or not 0 < s[1] <= CAPTURE_TICKS:
            return 'adc_samples'
        for _, base in channels:
            low, high, mean, value = s[base:base + 4]
            if not low <= mean <= high <= MAX_RAW or value != scaled(mean, reported[0], reported[1]):
                return 'adc_values'
    if supply and any(abs(s[16] - s[base + 2]) < DISTINCT_RAW for s in samples for base in (6, 10)):
        return 'adc_channels'
    if len(samples) < MIN_SAMPLES:
        return 'adc_samples'
    first, last = arrival(found[2][1].start()), arrival(found[-1][1].start())
    result['adc_sample_seconds'] = round((last - first).total_seconds() / (len(samples) - 1), 3)
    if not SAMPLE_SECONDS[0] <= result['adc_sample_seconds'] <= SAMPLE_SECONDS[1]:
        return 'adc_timing'
    return 'pass'


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    policy = manifest.get('startup_adc_policy')
    require(policy in POLICIES, 'ADC policy required')
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = {'outcome': 'pass', 'startups': len(re.findall(rb'PLFW1 start ', rx)), 'adc_enabled_before': None, 'adc_clock': None,
              'adc_calibration': None, 'adc_samples': None, 'adc_ticks_max': None, 'adc_capture_ms_max': None,
              'adc_sample_seconds': None, 'adc_food_levels': None, 'adc_outlet_levels': None, 'adc_dc_levels': None,
              'adc_current': None, 'adc_battery': None, 'adc_supply': None}
    check = STARTUP.startup(rx, manifest, image, identity, arrival, EVIDENCE.timestamp(metadata['end_utc']))
    check.pop('start_offset', None)
    result['startup'] = check
    found, intact = records(rx)
    if check['outcome'] != 'pass':
        result['outcome'] = f"startup_{check['outcome']}"
    elif not intact or any(f in rx for f in FOREIGN) or any(kind == 'E' for kind, _ in found):
        result['outcome'] = 'adc_sequence'
    else:
        result['outcome'] = check_adc(rx, found, arrival, result, policy == 'fw14-adc-v2')
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
