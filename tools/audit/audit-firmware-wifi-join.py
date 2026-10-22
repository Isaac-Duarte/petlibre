#!/usr/bin/env python3
"""Verify a finalized receive-only FW07 join capture against its build artifact.

The capture is one boot. A passing capture is one startup whose LED probe
completes, then the probe's events, numbered from 1 with none overwritten and
outputs stopped at every one:
  - wifi_on succeeds;
  - every attempt runs scan, then connect only when the network was found, then
    DHCP only when connect succeeded, then (when an address was assigned) a loss,
    then a backoff; the attempt number increases by one each time;
  - an assigned address is never 0.0.0.0 nor the SDK's static fallback 192.168.1.80;
  - backoffs double from 1 s to at most 30 s.
fw07-join-v1 (PLWIF2): backoffs reset after a join, and losses are link drops.
The operator switches the access point off after the join, then on again; a
pass needs a join, a loss after it, and a rejoin after that first loss.
fw07-dns-v1 (PLWIF3): a joined attempt runs DNS lookups before its loss. A loss
is a link drop, or a DNS loss after exactly DNS_FAILURES consecutive failed
lookups; backoffs reset after a successful lookup. The operator blocks DNS at the
access point after a lookup succeeds, then unblocks it; a pass needs a
successful lookup, a DNS loss after it, and a successful lookup after that loss.
The report gives the join, loss and recovery times; the address stays in this
private report. The SSID and password never appear in these records.
"""
import argparse
import importlib.util
import ipaddress
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('firmware_startup', ROOT / 'tools/audit/audit-firmware-startup.py')
STARTUP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STARTUP)
EVIDENCE = STARTUP.EVIDENCE
POLICIES = {'fw07-join-v1': b'PLWIF2', 'fw07-dns-v1': b'PLWIF3'}
FALLBACK = ipaddress.IPv4Address('192.168.1.80')
BACKOFF_MS = (1000, 30000)
DNS_FAILURES = 2  # example_entry.c PETLIBRE_DNS_FAILURES
DEC, SDEC, HEX8 = rb'(\d{1,10})', rb'(-?\d{1,10})', rb'([0-9a-f]{8})'
KINDS = ('on', 'scan', 'connect', 'dhcp', 'lost', 'backoff', 'dns')
FOREIGN = (b'PLFLS1', b'PLPRK1', b'PLRDS1', b'PLWRS1', b'PLJNL1', b'PLSET1', b'PLTMR1', b'PLADC1', b'PLWIF1')
LIMIT = ('UART evidence of the firmware\'s own join, DHCP, DNS, loss and backoff events; the access point\'s outage '
         'and DNS block instants are operator-chosen and not measured. Traffic beyond DHCP and DNS, flash and eFuse '
         'behavior at runtime and credential handling inside the SDK are not established by this capture.')
# lwip_netconf.h: DHCP_START=0, DHCP_WAIT_ADDRESS, DHCP_ADDRESS_ASSIGNED, DHCP_RELEASE_IP, DHCP_STOP, DHCP_TIMEOUT
DHCP_ASSIGNED = 2


def formats(prefix):
    return {
        'S': prefix + rb' start created=([01])\r\n',
        'E': (prefix + rb' ev seq=' + DEC + rb' kind=(' + b'|'.join(k.encode() for k in KINDS) + rb') attempt=' + DEC
              + rb' result=' + SDEC + rb' value=' + HEX8 + rb' rssi=' + SDEC + rb' security=' + HEX8 + rb' ms=' + DEC
              + rb' stopped=([01])\r\n'),
        'O': prefix + rb' error overrun seq=' + DEC + rb'\r\n',
    }


def records(rx, prefix):
    known = formats(prefix)
    pattern = re.compile(b'|'.join(b'(?P<%s>%s)' % (k.encode(), v) for k, v in known.items()))
    found = [(m.lastgroup, m, re.fullmatch(known[m.lastgroup], m[0]).groups()) for m in pattern.finditer(rx)]
    return found, len(found) == rx.count(prefix)


def check_attempt(attempt, last, dns):
    """The attempt's grammar; None when it is well formed."""
    kinds = ''.join('n' if e['kind'] == 'dns' else e['kind'][0] for e in attempt)  # s c d n l b
    scan = attempt[0]
    connect = next((e for e in attempt if e['kind'] == 'connect'), None)
    dhcp = next((e for e in attempt if e['kind'] == 'dhcp'), None)
    joined = scan['result'] == 0 and scan['value'] == 1 and connect and connect['result'] == 0 \
        and dhcp and dhcp['result'] == DHCP_ASSIGNED
    lookups = [e for e in attempt if e['kind'] == 'dns']
    if dns and joined:
        shape = re.fullmatch(r'scdn*(lb?)?', kinds) if last else re.fullmatch(r'scdn*lb', kinds)
    else:
        expected = 's'
        if scan['result'] == 0 and scan['value'] == 1:
            expected += 'c'
            if connect and connect['result'] == 0:
                expected += 'd'
                if joined:
                    expected += 'l'
        shape = kinds == expected + 'b' or (last and (expected + 'b').startswith(kinds))
    if not shape:
        return 'wifi_attempts'
    for e in lookups:
        if (e['result'] == 0) != (e['value'] != 0):
            return 'wifi_dns'
    trailing = 0
    for e in reversed(lookups):
        if e['result'] == 0:
            break
        trailing += 1
    lost = next((e for e in attempt if e['kind'] == 'lost'), None)
    if lost and lost['result'] not in ((0, 1) if dns else (0,)):
        return 'wifi_attempts'
    if lost and lost['result'] == 1:
        if trailing != DNS_FAILURES:
            return 'wifi_dns'
    elif trailing >= DNS_FAILURES:
        # A link loss, or still joined at the capture end, after recovery was due.
        return 'wifi_dns'
    return None


def check_join(rx, found, arrival, result, dns):
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
    attempts, joins, losses, backoffs, lookups = [], [], [], [], []
    previous, expected_backoff = 0, BACKOFF_MS[0]
    for e in events[1:]:
        if e['kind'] == 'on' or (e['kind'] == 'dns' and not dns):
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
        problem = check_attempt(attempt, attempt is attempts[-1], dns)
        if problem:
            return problem
        connect = next((e for e in attempt if e['kind'] == 'connect'), None)
        dhcp = next((e for e in attempt if e['kind'] == 'dhcp'), None)
        if dhcp and dhcp['result'] == DHCP_ASSIGNED:
            address = ipaddress.IPv4Address(dhcp['value'])
            if address in (ipaddress.IPv4Address(0), FALLBACK):
                return 'wifi_address'
            joins.append({'attempt': dhcp['attempt'], 'address': str(address), 'dhcp_ms': dhcp['ms'],
                          'connect_ms': connect['ms'], 'rssi': attempt[0]['rssi'], 'at': dhcp['at']})
            if not dns:
                expected_backoff = BACKOFF_MS[0]
        for e in attempt:
            if e['kind'] == 'dns':
                lookups.append(e)
                if e['result'] == 0:
                    expected_backoff = BACKOFF_MS[0]
        lost = next((e for e in attempt if e['kind'] == 'lost'), None)
        if lost:
            losses.append({'attempt': lost['attempt'], 'cause': 'dns' if lost['result'] else 'link',
                           'connected_ms': lost['value'], 'at': lost['at']})
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
    if not dns:
        if not losses or losses[0]['at'] < joins[0]['at']:
            return 'wifi_no_loss'
        # The operator's outage is the first loss; later ones are reported only.
        rejoin = next((j for j in joins if j['at'] > losses[0]['at']), None)
        if rejoin is None:
            return 'wifi_no_rejoin'
        result['wifi_rejoin_seconds_after_loss'] = round((rejoin['at'] - losses[0]['at']).total_seconds(), 2)
        return 'pass'
    ok = [e for e in lookups if e['result'] == 0]
    result['wifi_dns_lookups'] = len(lookups)
    result['wifi_dns_failures'] = len(lookups) - len(ok)
    result['wifi_dns_ms_max'] = max((e['ms'] for e in lookups), default=None)
    if not ok:
        return 'wifi_no_dns'
    # The operator's DNS block is the first DNS loss after a successful lookup.
    loss = next((x for x in losses if x['cause'] == 'dns' and x['at'] > ok[0]['at']), None)
    if loss is None:
        return 'wifi_no_dns_loss'
    recovered = next((e for e in ok if e['at'] > loss['at']), None)
    if recovered is None:
        return 'wifi_no_dns_recovery'
    result['wifi_dns_loss_seconds_after_first_lookup'] = round((loss['at'] - ok[0]['at']).total_seconds(), 2)
    result['wifi_dns_recovery_seconds_after_loss'] = round((recovered['at'] - loss['at']).total_seconds(), 2)
    return 'pass'


def verify(capture, manifest_path, image_path):
    require = EVIDENCE.require
    manifest, identity, image, metadata, rx, tx, events = STARTUP.load(capture, manifest_path, image_path)
    policy = manifest.get('startup_wifi_policy')
    require(policy in POLICIES, 'Wi-Fi join policy required')
    prefix, dns = POLICIES[policy], policy == 'fw07-dns-v1'
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = {'outcome': 'pass', 'startups': len(re.findall(rb'PLFW1 start ', rx)), 'wifi_on_ms': None,
              'wifi_attempts': None, 'wifi_backoffs_ms': None, 'wifi_joins': None, 'wifi_losses': None,
              'wifi_first_join_seconds': None, 'wifi_rejoin_seconds_after_loss': None}
    if dns:
        result.update({'wifi_dns_lookups': None, 'wifi_dns_failures': None, 'wifi_dns_ms_max': None,
                       'wifi_dns_loss_seconds_after_first_lookup': None, 'wifi_dns_recovery_seconds_after_loss': None})
    check = STARTUP.startup(rx, manifest, image, identity, arrival, EVIDENCE.timestamp(metadata['end_utc']))
    check.pop('start_offset', None)
    result['startup'] = check
    found, intact = records(rx, prefix)
    foreign = FOREIGN + tuple(p for p in POLICIES.values() if p != prefix)
    if check['outcome'] != 'pass':
        result['outcome'] = f"startup_{check['outcome']}"
    elif not intact or any(f in rx for f in foreign) or any(kind == 'O' for kind, _, _ in found):
        result['outcome'] = 'wifi_sequence'
    else:
        result['outcome'] = check_join(rx, found, arrival, result, dns)
    return {**result, 'policy': policy, 'build_id': identity, 'ota_sha256': STARTUP.digest(image), 'rx_sha256': STARTUP.digest(rx),
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
