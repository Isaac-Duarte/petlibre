#!/usr/bin/env python3
"""Verify a finalized receive-only FW07 scan capture against its build artifact.

A passing capture is one startup whose LED probe completes, then the scan probe:
the Wi-Fi task starts, wifi_on(STA) succeeds within 30 s, and three scans follow,
each successful with stopped outputs and exactly one record per kept network
(numbered from 0, RSSI and channel in range). Networks appear only as a CRC-32 of
their SSID. With --ssid-file (a private JSON file holding "ssid"), the report says
whether that network was seen and at what RSSI; the name is never written out.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import zlib

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_startup', ROOT / 'tools/audit-firmware-startup.py')
STARTUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STARTUP)
EVIDENCE = STARTUP.EVIDENCE
POLICY = 'fw07-scan-v1'
SCANS = 3
APS = 32  # networks kept per scan by the shim
ON_MS = 30000
SCAN_MS = 10000
CHANNELS = set(range(1, 15)) | set(range(36, 178))
DEC, SDEC = rb'(\d{1,10})', rb'(-?\d{1,10})'
RECORDS = {  # letter -> fixed record format
    'S': rb'PLWIF1 start created=([01])\r\n',
    'O': rb'PLWIF1 on result=' + SDEC + rb' ms=' + DEC + rb'\r\n',
    'C': (rb'PLWIF1 scan n=' + DEC + rb' result=' + SDEC + rb' ms=' + DEC + rb' count=' + DEC + rb' dropped=' + DEC
          + rb' stopped=([01])\r\n'),
    'A': (rb'PLWIF1 ap n=' + DEC + rb' i=' + DEC + rb' ssid_crc=([0-9a-f]{8}) rssi=' + SDEC + rb' channel=' + DEC
          + rb' security=([0-9a-f]{8})\r\n'),
    'D': rb'PLWIF1 done scans=' + DEC + rb'\r\n',
    'T': rb'PLWIF1 error timeout stage=' + DEC + rb' ms=' + DEC + rb'\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
FOREIGN = (b'PLFLS1', b'PLPRK1', b'PLRDS1', b'PLWRS1', b'PLJNL1', b'PLSET1', b'PLTMR1', b'PLADC1')
LIMIT = ('UART evidence of radio start and scan results as the firmware reported them; RSSI and channel are the '
         'driver\'s values. Association, DHCP, flash and eFuse behavior at runtime are not established by this capture.')


def records(rx):
    """Fixed-format Wi-Fi records; PLWIF1 text that does not parse is reported, not echoed."""
    found = [(m.lastgroup, m, re.fullmatch(RECORDS[m.lastgroup], m[0]).groups()) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLWIF1')


def check_wifi(rx, found, result, target=None):
    kinds = ''.join(kind for kind, _, _ in found)
    if not re.fullmatch(r'SO(?:CA*){%d}D' % SCANS, kinds):
        return 'wifi_timeout' if 'T' in kinds else ('wifi_on' if re.fullmatch(r'SO', kinds) else 'wifi_sequence')
    led_done = rx.find(b'PLLED1 done low=all\r\n')
    if led_done < 0 or found[0][1].start() < led_done:
        return 'wifi_sequence'
    if found[0][2] != (b'1',):
        return 'wifi_start'
    on_result, on_ms = (int(v) for v in found[1][2])
    result['wifi_on_ms'] = on_ms
    if on_result != 0 or not 0 < on_ms <= ON_MS:
        return 'wifi_on'
    scans, current = [], None
    for kind, _, fields in found[2:-1]:
        if kind == 'C':
            n, code, ms, count, dropped, stopped = (int(v) for v in fields)
            current = {'n': n, 'result': code, 'ms': ms, 'count': count, 'dropped': dropped, 'stopped': stopped, 'aps': []}
            scans.append(current)
        else:
            n, index, crc, rssi, channel, security = fields
            current['aps'].append((int(n), int(index), crc.decode(), int(rssi), int(channel), security.decode()))
    result['wifi_scans'] = [{k: v for k, v in s.items() if k != 'aps'} for s in scans]
    result['wifi_networks_max'] = max(s['count'] + s['dropped'] for s in scans)  # count is those kept
    for number, s in enumerate(scans, 1):
        if (s['n'], s['result'], s['stopped']) != (number, 0, 1) or not 0 < s['ms'] <= SCAN_MS:
            return 'wifi_scan'
        if s['count'] > APS or (s['dropped'] and s['count'] != APS) or len(s['aps']) != s['count']:
            return 'wifi_scan'
        for index, (n, i, _, rssi, channel, _) in enumerate(s['aps']):
            if (n, i) != (number, index) or not -127 <= rssi <= 0 or channel not in CHANNELS:
                return 'wifi_networks'
    done = int(found[-1][2][0])
    if done != SCANS:
        return 'wifi_sequence'
    if target is not None:
        crc = f'{zlib.crc32(target):08x}'
        seen = [max((rssi for _, _, c, rssi, _, _ in s['aps'] if c == crc), default=None) for s in scans]
        result['configured_network_rssi'] = seen
        result['configured_network_seen'] = any(v is not None for v in seen)
    return 'pass'


def verify(capture, manifest_path, image_path, ssid_file=None):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_wifi_policy') == POLICY, 'Wi-Fi policy required')
    target = None
    if ssid_file is not None:
        require(Path(ssid_file).resolve().is_relative_to((ROOT / 'private').resolve()), 'SSID file must stay under private/')
        target = json.loads(Path(ssid_file).read_text())['ssid'].encode()
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = {'outcome': 'pass', 'startups': len(re.findall(rb'PLFW1 start ', rx)), 'wifi_on_ms': None, 'wifi_scans': None,
              'wifi_networks_max': None, 'configured_network_seen': None, 'configured_network_rssi': None}
    check = STARTUP.startup(rx, manifest, image, identity, arrival, EVIDENCE.timestamp(metadata['end_utc']))
    check.pop('start_offset', None)
    result['startup'] = check
    found, intact = records(rx)
    if check['outcome'] != 'pass':
        result['outcome'] = f"startup_{check['outcome']}"
    elif not intact or any(f in rx for f in FOREIGN):
        result['outcome'] = 'wifi_sequence'
    else:
        result['outcome'] = check_wifi(rx, found, result, target)
    return {**result, 'build_id': identity, 'ota_sha256': STARTUP.digest(image), 'rx_sha256': STARTUP.digest(rx),
            'tx_bytes': len(tx), 'limit': LIMIT}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('capture', 'manifest', 'image', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--ssid-file', type=Path)
    args = parser.parse_args()
    if not args.output.resolve().is_relative_to((ROOT / 'private').resolve()):
        parser.error('Evidence output must stay under private/')
    report = verify(args.capture, args.manifest, args.image, args.ssid_file)
    with args.output.open('x') as stream:
        stream.write(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return 0 if report['outcome'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
