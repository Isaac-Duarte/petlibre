from datetime import timedelta
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


M = load('firmware_transport', TOOLS / 'audit-firmware-transport.py')
J = load('firmware_wifi_join_tests', TOOLS / 'tests/test_firmware_wifi_join.py')  # startup/LED capture helpers
S = M.STARTUP
DNS = b'PLWIF3 ev seq=5 kind=dns attempt=1 result=0 value=5db8d822 rssi=0 security=00000000 ms=3 stopped=1\r\n'


def stage_b():
    """(seconds, line): DNS ok, connect, ready, publishes, a ping and a health request."""
    return [(10, DNS), (11, b'PLMQT1 connect n=1 ok=1 ms=40\r\n'), (11.2, b'PLMQT1 ready ms=150\r\n'),
            (11.2, b'PLMQT1 publish seq=1 bytes=420 trigger=connect\r\n'),
            (20, b'PLMQT1 request op=ping ok=1\r\n'),
            (25, b'PLMQT1 request op=health ok=1\r\n'), (25, b'PLMQT1 publish seq=2 bytes=420 trigger=request\r\n'),
            (55, b'PLMQT1 publish seq=3 bytes=421 trigger=period\r\n'),
            (85, b'PLMQT1 publish seq=4 bytes=421 trigger=period\r\n')]


def stage_c():
    return stage_b()[:4] + [(30, b'PLMQT1 closed reason=silence\r\n'), (31, b'PLMQT1 connect n=2 ok=0 ms=3000\r\n'),
                            (40, b'PLMQT1 connect n=3 ok=1 ms=35\r\n'), (40.1, b'PLMQT1 ready ms=90\r\n'),
                            (40.1, b'PLMQT1 publish seq=2 bytes=430 trigger=connect\r\n'),
                            (50, b'PLMQT1 request op=unsupported ok=0\r\n')]


class FirmwareTransport(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy=None,
                             startup_wifi_policy='fw07-dns-v1', startup_transport_policy='fw08-transport-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT)
        self.image(self.base + b' ' + S.WIFI_DNS_MARK + b' ' + S.TRANSPORT_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def audit(self, lines, stage, start=b'PLMQT1 start created=1\r\n'):
        chunks = [(0.0, J.startup(self.identity)), (5.0, J.LED), (5.5, start)] + [(5.5 + t, d) for t, d in lines]
        chunks.sort(key=lambda c: c[0])
        finished = max(t for t, _ in chunks) + 5
        rx, events, offset = b''.join(c[1] for c in chunks), [], 0
        for t, data in chunks:
            events.append({'utc': (J.T0 + timedelta(seconds=t)).isoformat(), 'direction': 'rx', 'offset': offset, 'bytes': len(data)})
            offset += len(data)
        (self.p / 'serial-rx.bin').write_bytes(rx)
        (self.p / 'serial-tx.bin').write_bytes(b'')
        (self.p / 'serial-events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        (self.p / 'report.json').write_text(json.dumps({'mode': 'observe', 'status': 'complete', 'start_utc': J.T0.isoformat(),
                                                        'end_utc': (J.T0 + timedelta(seconds=finished)).isoformat(),
                                                        'serial-rx.bin_sha256': S.digest(rx), 'serial-tx.bin_sha256': S.digest(b'')}))
        return M.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin', stage)

    def test_stage_b_and_c_pass(self):
        report = self.audit(stage_b(), 'b')
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['transport_triggers'], {'connect': 1, 'period': 2, 'change': 0, 'request': 1})
        self.assertEqual(report['transport_connect_seconds_after_dns'], 1.0)
        report = self.audit(stage_c(), 'c')
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['transport_closes'], ['silence'])
        self.assertEqual(report['transport_reconnect_seconds'], 10.0)
        # Stage b refuses a close; stage c needs one.
        self.assertEqual(self.audit(stage_c(), 'b')['outcome'], 'transport_closed')
        self.assertEqual(self.audit(stage_b(), 'c')['outcome'], 'transport_no_reconnect')

    def test_failures_have_distinct_outcomes(self):
        b = stage_b()

        def later(steps, seconds):
            return [(t + seconds, d) for t, d in steps]

        def replace(index, line):
            steps = list(b)
            steps[index] = (steps[index][0], line)
            return steps
        cases = [
            (b[1:], 'transport_no_dns'),
            ([(10, DNS)] + later(b[1:], 19), 'transport_slow_connect'),
            (b[:2], 'transport_ready'),
            (b[:2] + later(b[2:], 14), 'transport_ready'),
            (replace(1, b'PLMQT1 connect n=2 ok=1 ms=40\r\n'), 'transport_sequence'),
            (replace(3, b'PLMQT1 publish seq=1 bytes=420 trigger=period\r\n'), 'transport_sequence'),
            (replace(7, b'PLMQT1 publish seq=4 bytes=421 trigger=period\r\n'), 'transport_publish'),
            (replace(7, b'PLMQT1 publish seq=3 bytes=600 trigger=period\r\n'), 'transport_publish'),
            (b[:7] + [(65, b[7][1]), (95, b[8][1])], 'transport_period'),
            (replace(4, b'PLMQT1 request op=ping ok=0\r\n'), 'transport_request'),
            (replace(4, b'PLMQT1 request op=unsupported ok=1\r\n'), 'transport_request'),
            (b[:6] + b[7:], 'transport_sequence'),
            (b[:4] + b[5:], 'transport_incomplete'),
            (b[:8], 'transport_incomplete'),
            ([(10, DNS), (11, b'PLMQT1 connect n=1 ok=0 ms=3000\r\n')], 'transport_no_connect'),
            (b + [(90, b'PLMQT1 error config\r\n')], 'transport_config'),
            (b + [(90, b'PLMQT1 publish seq=5 bytes=1 trigger=sideways\r\n')], 'transport_sequence'),
        ]
        for index, (lines, outcome) in enumerate(cases):
            with self.subTest(index=index, outcome=outcome):
                self.assertEqual(self.audit(lines, 'b')['outcome'], outcome)
        self.assertEqual(self.audit(b, 'b', start=b'PLMQT1 start created=0\r\n')['outcome'], 'transport_start')
        self.assertEqual(self.audit(stage_c()[:4] + stage_c()[4:5], 'c')['outcome'], 'transport_no_reconnect')

    def test_policy_binding_both_ways(self):
        self.manifest['startup_transport_policy'] = None
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, 'Transport policy required'):
            self.audit(stage_b(), 'b')
        self.manifest['startup_transport_policy'] = 'fw08-transport-v1'
        self.image(self.base + b' ' + S.WIFI_DNS_MARK)
        with self.assertRaisesRegex(ValueError, 'Transport policy/artifact mismatch'):
            self.audit(stage_b(), 'b')


if __name__ == '__main__':
    unittest.main()
