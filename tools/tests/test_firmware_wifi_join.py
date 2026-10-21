from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_wifi_join', TOOLS / 'audit-firmware-wifi-join.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
S = M.STARTUP
T0 = datetime(2026, 10, 21, 12, 0, tzinfo=timezone.utc)
POWER = b'PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r'
CLOCK = b'PLCLK1 function_before=001115f7 clock_before=00131557 function_after=001115f7 clock_after=00131557 status=pass\r\n'
OUTPUTS = b'PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n'
ROUTING = b''.join(f'PLIO1 stage={s} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001\r\n'.encode()
                   for s in ('startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22'))
LED = b'PLLED1 begin output_only=PA0,PA2,PA4\r\n' + b''.join(
    b'PLLED1 pin=' + pin + b' level=' + n + b' status=' + status + b'\r\n'
    for pin, status in ((b'PA0', b'output'), (b'PA2', b'output'), (b'PA4', b'output'), (b'PB22', b'pass')) for n in (b'1', b'0', b'1', b'0')) + b'PLLED1 done low=all\r\n'
KIND = {'on': 0, 'scan': 0, 'connect': 0, 'dhcp': 0, 'lost': 0, 'backoff': 0}
IP = 0x0a2a0005  # 10.42.0.5


def ev(seq, kind, attempt=0, result=0, value=0, rssi=0, ms=100, stopped=1):
    return b'PLWIF2 ev seq=%d kind=%s attempt=%d result=%d value=%08x rssi=%d security=%08x ms=%d stopped=%d\r\n' % (
        seq, kind.encode(), attempt, result, value, rssi, 0x00400004 if kind == 'scan' else 0, ms, stopped)


def script():
    """(seconds, kind, attempt, result, value): join, loss at the outage, two failed attempts, rejoin."""
    return [(0, 'on', 0, 0, 0), (4, 'scan', 1, 0, 1), (6, 'connect', 1, 0, 0), (8, 'dhcp', 1, 2, IP),
            (60, 'lost', 1, 0, 52000), (60, 'backoff', 1, 0, 1000),
            (65, 'scan', 2, 0, 0), (65, 'backoff', 2, 0, 2000),
            (71, 'scan', 3, 0, 0), (71, 'backoff', 3, 0, 4000),
            (80, 'scan', 4, 0, 1), (82, 'connect', 4, 0, 0), (84, 'dhcp', 4, 2, IP)]


def lines(steps=None, edit=None):
    steps = script() if steps is None else steps
    out = [(6.0, b'PLWIF2 start created=1\r\n')]
    out += [(7.0 + t, ev(n, k, a, r, v, -45 if k == 'scan' else 0)) for n, (t, k, a, r, v) in enumerate(steps, 1)]
    return edit(out) if edit else out


def capture(identity, events=None):
    events = lines() if events is None else events
    chunks = [(0.0, startup(identity)), (5.0, LED)] + events
    return chunks, max(t for t, _ in chunks) + 10


def startup(identity, reset=b'00000000'):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Petlibre local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


class FirmwareWifiJoin(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy=None,
                             startup_wifi_policy='fw07-join-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT)
        self.image(self.base + b' ' + S.WIFI_JOIN_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def policy(self, policy):
        self.manifest['startup_wifi_policy'] = policy
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def audit(self, received):
        chunks, finished = received
        chunks = sorted(chunks, key=lambda c: c[0])
        rx, events, offset = b''.join(c[1] for c in chunks), [], 0
        for t, data in chunks:
            events.append({'utc': (T0 + timedelta(seconds=t)).isoformat(), 'direction': 'rx', 'offset': offset, 'bytes': len(data)})
            offset += len(data)
        (self.p / 'serial-rx.bin').write_bytes(rx)
        (self.p / 'serial-tx.bin').write_bytes(b'')
        (self.p / 'serial-events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        (self.p / 'report.json').write_text(json.dumps({'mode': 'observe', 'status': 'complete', 'start_utc': T0.isoformat(),
                                                        'end_utc': (T0 + timedelta(seconds=finished)).isoformat(),
                                                        'serial-rx.bin_sha256': S.digest(rx), 'serial-tx.bin_sha256': S.digest(b'')}))
        return M.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin')

    def test_join_loss_and_rejoin_pass_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['wifi_attempts'], 4)
        self.assertEqual(report['wifi_backoffs_ms'], [1000, 2000, 4000])
        self.assertEqual([j['address'] for j in report['wifi_joins']], ['10.42.0.5'] * 2)
        self.assertEqual(report['wifi_losses'], [{'attempt': 1, 'connected_ms': 52000}])
        self.assertEqual(report['wifi_first_join_seconds'], 8.0)
        self.assertEqual(report['wifi_rejoin_seconds_after_loss'], 24.0)
        self.assertNotIn('PLWIF2', json.dumps(report))
        # Ending mid-attempt is fine.
        self.assertEqual(self.audit(capture(self.identity, lines(script() + [(90, 'lost', 4, 0, 6000)])))['outcome'], 'pass')
        self.assertEqual(self.audit(capture(self.identity, lines(script() + [(90, 'lost', 4, 0, 6000), (90, 'backoff', 4, 0, 1000),
                                                                             (91, 'scan', 5, 0, 1)])))['outcome'], 'pass')

    def test_failures_have_distinct_outcomes(self):
        base = script()

        def at(index, **change):
            steps = list(base)
            t, k, a, r, v = steps[index]
            steps[index] = (t, change.get('kind', k), change.get('attempt', a), change.get('result', r), change.get('value', v))
            return lines(steps)
        cases = [
            (at(0, result=-1)[:2], 'wifi_on'),
            (lines(base[:4]), 'wifi_no_loss'),
            (lines(base[:6]), 'wifi_no_rejoin'),
            (lines([base[0], (4, 'scan', 1, 0, 0), (4, 'backoff', 1, 0, 1000)]), 'wifi_no_join'),
            (at(3, value=0xc0a80150), 'wifi_address'),
            (at(3, value=0), 'wifi_address'),
            (at(7, value=3000), 'wifi_backoff'),
            (at(5, value=2000), 'wifi_backoff'),
            (at(6, attempt=3), 'wifi_attempts'),
            (at(2, attempt=2), 'wifi_attempts'),
            (lines(base[:1] + [(4, 'scan', 1, 0, 0), (5, 'connect', 1, 0, 0)] + base[4:]), 'wifi_attempts'),
            (lines(base[:3] + base[4:]), 'wifi_attempts'),
            (lines(base, edit=lambda o: o[:3] + o[4:]), 'wifi_sequence'),
            (lines(base, edit=lambda o: [(t, d.replace(b'stopped=1', b'stopped=0') if i == 5 else d) for i, (t, d) in enumerate(o)]),
             'wifi_sequence'),
            (lines(base, edit=lambda o: o + [(200, b'PLWIF2 error overrun seq=3\r\n')]), 'wifi_sequence'),
            (lines(base, edit=lambda o: [(t, d.replace(b'created=1', b'created=0')) for t, d in o]), 'wifi_start'),
            (lines(base, edit=lambda o: o + [(200, b'PLWIF1 start created=1\r\n')]), 'wifi_sequence'),
            (lines(base + [(100, 'on', 0, 0, 0)]), 'wifi_sequence'),
        ]
        for index, (events, outcome) in enumerate(cases):
            with self.subTest(index=index, outcome=outcome):
                self.assertEqual(self.audit(capture(self.identity, events))['outcome'], outcome)
        chunks, finished = capture(self.identity)
        chunks.append((finished - 1, startup(self.identity)))
        self.assertEqual(self.audit((chunks, finished))['outcome'], 'startup_repeated_startup')

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        self.policy('fw07-scan-v1')
        with self.assertRaisesRegex(ValueError, 'Wi-Fi join policy required'):
            self.audit(good)
        self.manifest['startup_wifi_policy'] = 'fw07-join-v1'
        self.image(self.base + b' ' + S.WIFI_MARK)
        with self.assertRaisesRegex(ValueError, 'Wi-Fi'):
            self.audit(good)


if __name__ == '__main__':
    unittest.main()
