#!/usr/bin/env python3
"""Verify a finalized receive-only FW08 transport capture against its build artifact.

A passing capture is one startup whose LED probe completes, then
`PLMQT1 start created=1` and the transport's records in their fixed formats:
  - connect attempts numbered from 1; a successful one is followed by `ready`
    within 10 s, then a publish with trigger=connect;
  - publish sequence numbers increase by one across the whole boot;
  - while connected, period publishes come 30 s (+-5 s) after the previous publish;
  - each `request op=health ok=1` is followed by a publish with trigger=request;
  - the first successful connect comes within 15 s of the first successful
    FW07 DNS lookup (`PLWIF3 ev ... kind=dns ... result=0`).
Stage b additionally needs no close, at least two period publishes, and an
answered ping and health request. Stage c needs a close followed by a
reconnect, ready and connect-triggered publish, and a refused unsupported
request. The report never contains credentials; the capture has none.
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
POLICY = 'fw08-transport-v1'
DEC = rb'(\d{1,10})'
REASONS = (b'refused', b'subscribe_refused', b'protocol_too_long', b'protocol_malformed', b'protocol_unexpected',
           b'out_of_order', b'reply_timeout', b'silence', b'foreign_topic', b'wifi', b'closed', b'send', b'health_format')
RECORDS = {
    'start': rb'PLMQT1 start created=([01])\r\n',
    'connect': rb'PLMQT1 connect n=' + DEC + rb' ok=([01]) ms=' + DEC + rb'\r\n',
    'ready': rb'PLMQT1 ready ms=' + DEC + rb'\r\n',
    'publish': rb'PLMQT1 publish seq=' + DEC + rb' bytes=' + DEC + rb' trigger=(connect|period|change|request)\r\n',
    'request': rb'PLMQT1 request op=(health|ping|unsupported|malformed) ok=([01])\r\n',
    'closed': rb'PLMQT1 closed reason=(' + b'|'.join(REASONS) + rb')\r\n',
    'error': rb'PLMQT1 error config\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
DNS_OK = re.compile(rb'PLWIF3 ev seq=\d+ kind=dns attempt=\d+ result=0 ')
READY_S, CONNECT_AFTER_DNS_S, PERIOD_S, PERIOD_SLACK_S, MAX_BYTES = 10, 15, 30, 5, 512
LIMIT = ('UART evidence of the firmware\'s own transport records; what the broker and server received is checked '
         'separately against the server\'s status. Timing carries USB latency.')


def records(rx):
    found = [(m.lastgroup, re.fullmatch(RECORDS[m.lastgroup], m[0]).groups(), m.start()) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLMQT1')


def check(rx, found, arrival, stage, result):
    if not found or found[0][0] != 'start' or any(kind == 'start' for kind, _, _ in found[1:]):
        return 'transport_sequence'
    led_done = rx.find(b'PLLED1 done low=all\r\n')
    if led_done < 0 or found[0][2] < led_done:
        return 'transport_sequence'
    if found[0][1] != (b'1',):
        return 'transport_start'
    if any(kind == 'error' for kind, _, _ in found):
        return 'transport_config'
    events = [{'kind': kind, 'fields': [f.decode() for f in fields], 'at': arrival(offset)} for kind, fields, offset in found[1:]]
    connects, publishes, closes, requests = [], [], [], []
    connected, expect, last_publish, seq = False, None, None, 0
    for e in events:
        kind, f, at = e['kind'], e['fields'], e['at']
        # A close may interrupt anything (a send can fail at any point).
        if expect and kind not in (expect[0], 'closed'):
            return 'transport_sequence'
        if kind == 'connect':
            if connected or int(f[0]) != len(connects) + 1:
                return 'transport_sequence'
            connects.append({'n': int(f[0]), 'ok': f[1] == '1', 'ms': int(f[2]), 'at': at})
            expect = ('ready', at) if f[1] == '1' else None
            connected = f[1] == '1'
        elif kind == 'ready':
            if not expect or expect[0] != 'ready':
                return 'transport_sequence'
            if (at - expect[1]).total_seconds() > READY_S:
                return 'transport_ready'
            expect = ('publish', at, 'connect')
        elif kind == 'publish':
            number, size, trigger = int(f[0]), int(f[1]), f[2]
            if not connected or (expect and expect[0] == 'publish' and trigger != expect[2]):
                return 'transport_sequence'
            if number != seq + 1 or not 0 < size <= MAX_BYTES:
                return 'transport_publish'
            if trigger == 'period' and last_publish is not None:
                gap = (at - last_publish).total_seconds()
                if abs(gap - PERIOD_S) > PERIOD_SLACK_S:
                    return 'transport_period'
            seq, last_publish, expect = number, at, None
            publishes.append({'seq': number, 'bytes': size, 'trigger': trigger})
        elif kind == 'request':
            if not connected:
                return 'transport_sequence'
            requests.append({'op': f[0], 'ok': f[1] == '1'})
            if (f[0] in ('health', 'ping')) != (f[1] == '1'):
                return 'transport_request'
            expect = ('publish', at, 'request') if f[0] == 'health' else None
        elif kind == 'closed':
            if not connected:
                return 'transport_sequence'
            closes.append({'reason': f[0], 'at': at})
            connected, last_publish, expect = False, None, None
    result.update(transport_connects=len(connects), transport_publishes=len(publishes),
                  transport_closes=[c['reason'] for c in closes], transport_requests=requests,
                  transport_triggers={t: sum(p['trigger'] == t for p in publishes) for t in ('connect', 'period', 'change', 'request')})
    if expect and expect[0] == 'ready':
        return 'transport_ready'
    first = next((c for c in connects if c['ok']), None)
    if first is None:
        return 'transport_no_connect'
    dns = DNS_OK.search(rx)
    if dns is None:
        return 'transport_no_dns'
    after_dns = (first['at'] - arrival(dns.start())).total_seconds()
    result['transport_connect_seconds_after_dns'] = round(after_dns, 2)
    if after_dns > CONNECT_AFTER_DNS_S:
        return 'transport_slow_connect'
    triggers = result['transport_triggers']
    ops = {(r['op'], r['ok']) for r in requests}
    if stage == 'b':
        if closes:
            return 'transport_closed'
        if triggers['period'] < 2 or not {('ping', True), ('health', True)} <= ops:
            return 'transport_incomplete'
    else:
        reconnect = closes and next((c for c in connects if c['ok'] and c['at'] > closes[0]['at']), None)
        if not reconnect or triggers['connect'] < 2:
            return 'transport_no_reconnect'
        result['transport_reconnect_seconds'] = round((reconnect['at'] - closes[0]['at']).total_seconds(), 2)
        if ('unsupported', False) not in ops:
            return 'transport_incomplete'
    return 'pass'


def verify(capture, manifest_path, image_path, stage):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_transport_policy') == POLICY, 'Transport policy required')
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
        result['outcome'] = 'transport_sequence'
    else:
        result['outcome'] = check(rx, found, arrival, stage, result)
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
