#!/usr/bin/env python3
"""Verify finalized receive-only diagnostic startup against its build artifact."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('startup_evidence', ROOT / 'tools/audit-uart-startups.py')
EVIDENCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVIDENCE)
START = re.compile(rb'PLFW1 start build=([0-9a-f]{64}) bank=(0|1|unknown) reset=([0-9a-f]{8}) mmu=([0-9a-f]{8}),([0-9a-f]{8}),([0-9a-f]{8})\r\n')
PASS = b'PLFW1 result diagnostic=pass backend=unopened\r\n'
FAIL = b'PLFW1 result diagnostic=fail backend=unopened\r\n'
OUTPUTS = re.compile(rb'PLFW1 outputs pa=([0-9a-f]{8}) pb=([0-9a-f]{8}) dira=([0-9a-f]{8}) dirb=([0-9a-f]{8}) status=(pass|fail)\r\n')
CLOCK = re.compile(rb'PLCLK1 function_before=([0-9a-f]{8}) clock_before=([0-9a-f]{8}) function_after=([0-9a-f]{8}) clock_after=([0-9a-f]{8}) status=(pass|fail)\r\n')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def classify_clock(rx):
    values = list(CLOCK.finditer(rx))
    if len(values) != 1 or rx.count(b'PLCLK1') != 1:
        return {'outcome': 'gpio_clock_missing' if not values else 'gpio_clock_repeated', 'gpio_clock_verified': False}
    m = values[0]
    before_function, before_clock, function, clock = [int(v, 16) for v in m.groups()[:4]]
    bit = 1 << 20
    ok = bool(function & bit and clock & bit) and function & ~bit == before_function & ~bit and clock & ~bit == before_clock & ~bit and m[5] == b'pass'
    return {'outcome': 'pass' if ok else 'gpio_clock_failed', 'gpio_clock_verified': ok,
            'gpio_function_before': m[1].decode(), 'gpio_clock_before': m[2].decode(),
            'gpio_function_after': m[3].decode(), 'gpio_clock_after': m[4].decode()}


def classify_routing(rx):
    """Keep fixed numeric observations without inferring pin-group membership."""
    records = []
    for m in re.finditer(rb'PLIO1 stage=(startup|PA0|PA2|PA4|PB22) groups=([0-9a-f]{8}) aon=([0-9a-f]{8}) control_a=([0-9a-f]{8}) control_b=([0-9a-f]{8})\r\n', rx):
        records.append(dict(zip(('stage', 'groups', 'aon', 'control_a', 'control_b'), (v.decode() for v in m.groups()))))
    stages = [r['stage'] for r in records]
    expected = ['startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22']
    complete = stages == expected and rx.count(b'PLIO1') == len(records)
    return {'outcome': 'pass' if complete else 'gpio_routing_incomplete',
            'gpio_routing_complete': complete, 'gpio_routing_records': records}


def classify_led(rx):
    """Independent expected bounded GPIO sequence; no installer/firmware imports."""
    expected = [b'PLLED1 begin\r\n']
    for pin in (b'PA0', b'PA2', b'PA4', b'PB22'):
        for level in (b'1', b'0', b'1', b'0'):
            expected.append(b'PLLED1 pin=' + pin + b' level=' + level + b' status=pass\r\n')
    expected.append(b'PLLED1 done low=all\r\n')
    actual = re.findall(rb'PLLED1[^\r\n]*\r\n', rx)
    if b'PLLED1 error ' in rx or re.search(rb'PLLED1 pin=[^\r\n]* status=fail\r\n', rx):
        outcome = 'led_failed'
    elif not actual and b'PLLED1' not in rx:
        outcome = 'led_missing'
    elif len(actual) != rx.count(b'PLLED1') or actual != expected[:len(actual)]:
        outcome = 'led_unexpected_sequence'
    elif actual != expected:
        outcome = 'led_incomplete'
    else:
        outcome = 'pass'
    diagnostics = []
    for m in re.finditer(rb'PLDBG1 pin=(PA0|PA2|PA4|PB22) stage=(request|verify|initialize) want=([01]) latch=([01]) input=([01]) directions=([01]) stopped=([01]) pad=([0-9a-f]{8})\r\n', rx):
        diagnostics.append({'pin': m[1].decode(), 'stage': m[2].decode(),
                            **{key: bool(int(m[index])) for index, key in enumerate(
                                ('want_high', 'latch_high', 'input_high', 'directions_ok', 'stopped_ok'), 3)},
                            'pad_control': m[8].decode()})
    return {'outcome': outcome, 'led_probe_verified': outcome == 'pass', 'led_readbacks': diagnostics}


def classify(rx, identity, require_stopped=False):
    """Parse only fixed public fields; never return arbitrary device text."""
    starts = list(START.finditer(rx))
    outputs = list(OUTPUTS.finditer(rx))
    stopped = len(outputs) == 1 and tuple(int(v, 16) for v in outputs[0].groups()[:4]) == (0x6000, 0, 0xe000, 8) and outputs[0][5] == b'pass'
    outcome = 'timeout'
    if any(x in rx for x in (b'Hard Fault', b'HardFault', b'Fault on thread')):
        outcome = 'fault'
    elif rx.count(b'PLFW1 start ') > 1 or rx.count(b'Petlibro local firmware: Rust bringup') > 1:
        outcome = 'repeated_startup'
    elif outputs and not stopped or b'PLFW1 error outputs=' in rx:
        outcome = 'outputs_failed'
    elif b'PLFW1 error ' in rx or FAIL in rx or b'Synthetic core diagnostic: FAIL' in rx:
        outcome = 'diagnostic_failed'
    elif rx.count(b'PLFW1 ') != len(starts) + len(outputs) + rx.count(PASS) or rx.count(PASS) > 1:
        outcome = 'malformed_report'
    elif len(starts) == 1:
        m = starts[0]
        bank, reason, ctrl, start, offset = [int(x, 16) if x != b'unknown' else None for x in m.groups()[1:]]
        physical = start - offset if ctrl & 2 else start + offset
        live_bank = {0x08006000: 0, 0x08186000: 1}.get(physical) if ctrl & 1 and start == 0x0c000000 and 0 <= physical <= 0xffffffff else None
        if m[1].decode() != identity:
            outcome = 'wrong_build'
        elif bank is None or live_bank != bank:
            outcome = 'unknown_mapping'
        elif require_stopped and not stopped:
            outcome = 'outputs_missing'
        elif stopped and outputs[0].end() > m.start():
            outcome = 'outputs_after_startup'
        elif rx.count(PASS) == 1 and rx.index(PASS) >= m.end() and rx.count(b'Synthetic core diagnostic: PASS') == 1 and rx.count(b'Synthetic diagnostic only; feeder backend stays unopened.') == 1 and b'Flash Download Start' not in rx and b'app_main:version:' not in rx:
            outcome = 'pass'
        else:
            outcome = 'incomplete_report'
        return {'outcome': outcome, 'bank': bank, 'reset_flags': f'{reason:08x}', 'start_offset': m.start(), 'stopped_gpio_verified': stopped}
    return {'outcome': outcome}


def verify(capture, manifest_path, image_path, minimum_seconds):
    require = EVIDENCE.require
    manifest = json.loads(manifest_path.read_bytes())
    identity = digest(json.dumps({'profile': manifest['profile'], 'inputs': manifest['inputs']}, sort_keys=True, separators=(',', ':')).encode())
    require(manifest.get('startup_identity') == identity, 'Build identity is not source-bound')
    image = image_path.read_bytes()
    ota = [a for a in manifest['artifacts'] if Path(a['path']).name == 'OTA_All.bin']
    require(len(ota) == 1 and ota[0]['sha256'] == digest(image) and ota[0]['bytes'] == len(image), 'Artifact/manifest mismatch')
    require(f'PLFW1 start build={identity} '.encode() in image, 'Identity absent from artifact')
    metadata = json.loads((capture / 'report.json').read_bytes())
    rx, tx = [(capture / name).read_bytes() for name in ('serial-rx.bin', 'serial-tx.bin')]
    events = [json.loads(line) for line in (capture / 'serial-events.jsonl').read_text().splitlines()]
    EVIDENCE.analyze(rx, tx, events, metadata)
    require(not tx and metadata.get('mode') == 'observe', 'Receive-only observation required')
    policy = manifest.get('startup_output_policy')
    require(policy in (None, 'stop-only'), 'Unknown startup output policy')
    require((policy == 'stop-only') == (b'PLFW1 outputs pa=' in image), 'Output policy/artifact mismatch')
    result = classify(rx, identity, require_stopped=policy == 'stop-only')
    clock_policy = manifest.get('startup_gpio_clock_policy')
    require(clock_policy in (None, 'explicit-enable-v1'), 'Unknown GPIO clock policy')
    require(not clock_policy or policy == 'stop-only', 'Clock profile requires stopped-output reporting')
    require((clock_policy == 'explicit-enable-v1') == (b'PLCLK1 function_before=' in image), 'GPIO clock policy/artifact mismatch')
    if clock_policy:
        clock_result = classify_clock(rx)
        if result['outcome'] == 'pass':
            if clock_result['outcome'] == 'pass' and CLOCK.search(rx).end() > OUTPUTS.search(rx).start():
                clock_result.update(outcome='gpio_clock_order', gpio_clock_verified=False)
            result.update(clock_result)
        else:
            result.update({k: v for k, v in clock_result.items() if k != 'outcome'})
    led_policy = manifest.get('startup_led_policy')
    require(led_policy in (None, 'bounded-probe-v1'), 'Unknown startup LED policy')
    require((led_policy == 'bounded-probe-v1') == (b'PLLED1 begin' in image), 'LED policy/artifact mismatch')
    if led_policy:
        led_result = classify_led(rx)
        if result['outcome'] == 'pass':
            if led_result['outcome'] == 'pass' and rx.index(b'PLLED1 begin') < rx.index(PASS) + len(PASS):
                led_result = {'outcome': 'led_unexpected_sequence', 'led_probe_verified': False, 'led_readbacks': led_result['led_readbacks']}
            result.update(led_result)
        else:
            result.update(led_readbacks=led_result['led_readbacks'], led_probe_verified=False, led_outcome=led_result['outcome'])
    routing_policy = manifest.get('startup_gpio_routing_policy')
    require(routing_policy in (None, 'read-only-v1'), 'Unknown GPIO routing policy')
    require((routing_policy == 'read-only-v1') == (b'PLIO1 stage=' in image), 'GPIO routing policy/artifact mismatch')
    if routing_policy:
        routing = classify_routing(rx)
        if result['outcome'] == 'pass':
            result.update(routing)
        else:
            result.update({k: v for k, v in routing.items() if k != 'outcome'})
    if result['outcome'] == 'pass':
        at = result.pop('start_offset')
        received = next(e for e in events if e['direction'] == 'rx' and e['offset'] <= at < e['offset'] + e['bytes'])
        seconds = (EVIDENCE.timestamp(metadata['end_utc']) - EVIDENCE.timestamp(received['utc'])).total_seconds()
        result['seconds_after_startup'] = seconds
        if seconds < minimum_seconds:
            result['outcome'] = 'observation_too_short'
    result.pop('start_offset', None)
    return {**result, 'build_id': identity, 'ota_sha256': digest(image), 'rx_sha256': digest(rx), 'tx_bytes': len(tx), 'minimum_seconds': minimum_seconds,
            'limit': 'UART startup evidence; electrical outputs and physical reset cause are not established.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('capture', 'manifest', 'image', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--minimum-seconds', type=float, default=60)
    args = parser.parse_args()
    if not 1 <= args.minimum_seconds <= 180:
        parser.error('Observation minimum must be 1–180 seconds')
    if not args.output.resolve().is_relative_to((ROOT / 'private').resolve()):
        parser.error('Evidence output must stay under private/')
    report = verify(args.capture, args.manifest, args.image, args.minimum_seconds)
    with args.output.open('x') as stream:
        stream.write(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return 0 if report['outcome'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
