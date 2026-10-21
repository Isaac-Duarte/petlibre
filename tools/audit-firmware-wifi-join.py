#!/usr/bin/env python3
"""Verify a finalized receive-only FW07 join-and-reconnect capture against its build artifact.

The capture is one boot during which the operator switches the configured access
point off after the feeder has joined, then on again. A passing capture is one
startup whose LED probe completes, then the join probe's events, numbered from 1
with none overwritten and outputs stopped at every one:
  - wifi_on succeeds;
  - every attempt runs scan, then connect only when the network was found, then
    DHCP only when connect succeeded, then (when an address was assigned) a loss,
    then a backoff; the attempt number increases by one each time;
  - an assigned address is never 0.0.0.0 nor the SDK's static fallback 192.168.1.80;
  - backoffs double from 1 s to at most 30 s and reset to 1 s after a join;
  - there is a join, a loss after it, and a rejoin after that first loss.
The report gives the join, loss and rejoin times; the address stays in this private
report. The SSID and password never appear in these records.
"""
import argparse
import importlib.util
import ipaddress
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_startup', ROOT / 'tools/audit-firmware-startup.py')
STARTUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STARTUP)
EVIDENCE = STARTUP.EVIDENCE
POLICY = 'fw07-join-v1'
FALLBACK = ipaddress.IPv4Address('192.168.1.80')
BACKOFF_MS = (1000, 30000)
DEC, SDEC, HEX8 = rb'(\d{1,10})', rb'(-?\d{1,10})', rb'([0-9a-f]{8})'
KINDS = ('on', 'scan', 'connect', 'dhcp', 'lost', 'backoff')
RECORDS = {
    'S': rb'PLWIF2 start created=([01])\r\n',
    'E': (rb'PLWIF2 ev seq=' + DEC + rb' kind=(' + b'|'.join(k.encode() for k in KINDS) + rb') attempt=' + DEC
          + rb' result=' + SDEC + rb' value=' + HEX8 + rb' rssi=' + SDEC + rb' security=' + HEX8 + rb' ms=' + DEC
          + rb' stopped=([01])\r\n'),
    'O': rb'PLWIF2 error overrun seq=' + DEC + rb'\r\n',
}
ANY = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in RECORDS.items()))
FOREIGN = (b'PLFLS1', b'PLPRK1', b'PLRDS1', b'PLWRS1', b'PLJNL1', b'PLSET1', b'PLTMR1', b'PLADC1', b'PLWIF1')
LIMIT = ('UART evidence of the firmware\'s own join, DHCP, loss and backoff events; the access point\'s outage '
         'instants are operator-chosen and not measured. Traffic beyond DHCP, flash and eFuse behavior at runtime '
         'and credential handling inside the SDK are not established by this capture.')
# lwip_netconf.h: DHCP_START=0, DHCP_WAIT_ADDRESS, DHCP_ADDRESS_ASSIGNED, DHCP_RELEASE_IP, DHCP_STOP, DHCP_TIMEOUT
DHCP_ASSIGNED = 2


def records(rx):
    found = [(m.lastgroup, m, re.fullmatch(RECORDS[m.lastgroup], m[0]).groups()) for m in ANY.finditer(rx)]
    return found, len(found) == rx.count(b'PLWIF2')


def check_join(rx, found, arrival, result):
    if not found or found[0][0] != 'S' or any(kind != 'E' for kind, _, _ in found[1:]):
        return 'wifi_sequence'
    led_done = rx.find(b'PLLED1 done low=all\r\n')
    if led_done < 0 or found[0][1].start() < led_done:
        return 'wifi_sequence'
    if found[0][2] != (b'1',):
        return 'wifi_start'
    events = []
    for _, m, (seq, kind, attempt, code, value, rssi, security, ms, stopped) in found[1:]:
        events.append({'seq': int(seq), 'kind': kind.decode(), 'attempt': int(attempt), 'result': int(code),
                       'value': int(value, 16), 'rssi': int(rssi), 'ms': int(ms), 'stopped': int(stopped),
                       'at': arrival(m.start())})
    if [e['seq'] for e in events] != list(range(1, len(events) + 1)) or any(e['stopped'] != 1 for e in events):
        return 'wifi_sequence'
    if not events or events[0]['kind'] != 'on':
        return 'wifi_sequence'
    result['wifi_on_ms'] = events[0]['ms']
    if events[0]['result'] != 0:
        return 'wifi_on'
    attempts, joins, losses, backoffs = [], [], [], []
    previous, expected_backoff = 0, BACKOFF_MS[0]
    for e in events[1:]:
        if e['kind'] == 'on':
            return 'wifi_sequence'
        if e['kind'] == 'scan':
            if e['attempt'] != previous + 1:
                return 'wifi_attempts'
            previous = e['attempt']
            attempts.append([e])
            continue
        if not attempts or e['attempt'] != previous:
            return 'wifi_attempts'
        attempts[-1].append(e)
    for attempt in attempts:
        kinds = ''.join(e['kind'][0] for e in attempt)  # s c d l b
        scan = attempt[0]
        connect = next((e for e in attempt if e['kind'] == 'connect'), None)
        dhcp = next((e for e in attempt if e['kind'] == 'dhcp'), None)
        expected = 's'
        if scan['result'] == 0 and scan['value'] == 1:
            expected += 'c'
            if connect and connect['result'] == 0:
                expected += 'd'
                if dhcp and dhcp['result'] == DHCP_ASSIGNED:
                    expected += 'l'
        # Only the last attempt may still be in progress when the capture ends.
        if kinds != expected + 'b' and not (attempt is attempts[-1] and (expected + 'b').startswith(kinds)):
            return 'wifi_attempts'
        if dhcp and dhcp['result'] == DHCP_ASSIGNED:
            address = ipaddress.IPv4Address(dhcp['value'])
            if address in (ipaddress.IPv4Address(0), FALLBACK):
                return 'wifi_address'
            joins.append({'attempt': dhcp['attempt'], 'address': str(address), 'dhcp_ms': dhcp['ms'],
                          'connect_ms': connect['ms'], 'rssi': scan['rssi'], 'at': dhcp['at']})
            expected_backoff = BACKOFF_MS[0]
        lost = next((e for e in attempt if e['kind'] == 'lost'), None)
        if lost:
            losses.append({'attempt': lost['attempt'], 'connected_ms': lost['value'], 'at': lost['at']})
        backoff = next((e for e in attempt if e['kind'] == 'backoff'), None)
        if backoff:
            if backoff['value'] != expected_backoff:
                return 'wifi_backoff'
            backoffs.append(backoff['value'])
            expected_backoff = min(expected_backoff * 2, BACKOFF_MS[1])
    result['wifi_attempts'] = len(attempts)
    result['wifi_backoffs_ms'] = backoffs
    result['wifi_joins'] = [{k: v for k, v in j.items() if k != 'at'} for j in joins]
    result['wifi_losses'] = [{k: v for k, v in x.items() if k != 'at'} for x in losses]
    if not joins:
        return 'wifi_no_join'
    result['wifi_first_join_seconds'] = round((joins[0]['at'] - events[0]['at']).total_seconds(), 2)
    if not losses or losses[0]['at'] < joins[0]['at']:
        return 'wifi_no_loss'
    # The operator's outage is the first loss; later ones are reported only.
    rejoin = next((j for j in joins if j['at'] > losses[0]['at']), None)
    if rejoin is None:
        return 'wifi_no_rejoin'
    result['wifi_rejoin_seconds_after_loss'] = round((rejoin['at'] - losses[0]['at']).total_seconds(), 2)
    return 'pass'


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    require(manifest.get('startup_wifi_policy') == POLICY, 'Wi-Fi join policy required')
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = {'outcome': 'pass', 'startups': len(re.findall(rb'PLFW1 start ', rx)), 'wifi_on_ms': None,
              'wifi_attempts': None, 'wifi_backoffs_ms': None, 'wifi_joins': None, 'wifi_losses': None,
              'wifi_first_join_seconds': None, 'wifi_rejoin_seconds_after_loss': None}
    check = STARTUP.startup(rx, manifest, image, identity, arrival, EVIDENCE.timestamp(metadata['end_utc']))
    check.pop('start_offset', None)
    result['startup'] = check
    found, intact = records(rx)
    if check['outcome'] != 'pass':
        result['outcome'] = f"startup_{check['outcome']}"
    elif not intact or any(f in rx for f in FOREIGN) or any(kind == 'O' for kind, _, _ in found):
        result['outcome'] = 'wifi_sequence'
    else:
        result['outcome'] = check_join(rx, found, arrival, result)
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
