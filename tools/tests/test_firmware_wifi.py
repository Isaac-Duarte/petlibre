from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_wifi', TOOLS / 'audit/audit-firmware-wifi.py')
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
import zlib

SSID = b'bench-hotspot'


def ap(n, i, crc=0x12345678, rssi=-55, channel=6):
    return b'PLWIF1 ap n=%d i=%d ssid_crc=%08x rssi=%d channel=%d security=00400004\r\n' % (n, i, crc, rssi, channel)


def scan(n, count=3, result=0, ms=3100, dropped=0, stopped=1):
    return b'PLWIF1 scan n=%d result=%d ms=%d count=%d dropped=%d stopped=%d\r\n' % (n, result, ms, count, dropped, stopped)


def wifi(scans=3, count=3, on=b'PLWIF1 on result=0 ms=2100\r\n', target=True, edit=None):
    out = [b'PLWIF1 start created=1\r\n', on]
    for n in range(1, scans + 1):
        lines = [scan(n, count)] + [ap(n, i, zlib.crc32(SSID) if (target and i == 1) else 0x1000 + i, -40 - i) for i in range(min(count, 32))]
        out += lines
    out.append(b'PLWIF1 done scans=%d\r\n' % scans)
    if edit:
        out = edit(out)
    return out


def capture(identity, lines=None):
    lines = wifi() if lines is None else lines
    chunks = [(0.0, startup(identity)), (5.0, LED)]
    chunks += [(6.0 + 0.5 * i, line) for i, line in enumerate(lines)]
    return chunks, 6.0 + 0.5 * len(lines) + 5


def startup(identity, reset=b'00000000'):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Petlibre local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


class FirmwareWifi(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy=None,
                             startup_wifi_policy='fw07-scan-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT)
        self.image(self.base + b' ' + S.WIFI_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def policy(self, policy):
        self.manifest['startup_wifi_policy'] = policy
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def audit(self, received, ssid_file=None):
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
        return M.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin', ssid_file)

    def test_scan_capture_passes_and_reports_the_configured_network(self):
        ssid = Path(self.tmp.name).resolve()
        private = Path(__file__).resolve().parents[2] / 'private'
        private.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=private) as directory:
            path = Path(directory) / 'hotspot.json'
            path.write_text(json.dumps({'ssid': SSID.decode(), 'password': 'x' * 12}))
            report = self.audit(capture(self.identity), path)
            self.assertEqual(report['outcome'], 'pass', report)
            self.assertTrue(report['configured_network_seen'])
            self.assertEqual(report['configured_network_rssi'], [-41, -41, -41])
            self.assertNotIn(SSID.decode(), json.dumps(report))
            self.assertNotIn('xxxxxxxxxxxx', json.dumps(report))
            report = self.audit(capture(self.identity, wifi(target=False)), path)
            self.assertEqual(report['outcome'], 'pass')
            self.assertFalse(report['configured_network_seen'])
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['wifi_on_ms'], 2100)
        self.assertEqual(report['wifi_networks_max'], 3)
        self.assertIsNone(report['configured_network_seen'])
        self.assertNotIn('PLWIF1', json.dumps(report))
        # No networks at all, and more networks than the shim keeps.
        self.assertEqual(self.audit(capture(self.identity, wifi(count=0, target=False)))['outcome'], 'pass')
        many = wifi(count=32, edit=lambda o: [l.replace(b'count=32 dropped=0', b'count=32 dropped=8') for l in o])
        report = self.audit(capture(self.identity, many))
        self.assertEqual((report['outcome'], report['wifi_networks_max']), ('pass', 40))
        with self.assertRaisesRegex(ValueError, 'under private'):
            self.audit(capture(self.identity), ssid)

    def test_failures_have_distinct_outcomes(self):
        def change(old, new, index=None):
            return lambda o: [l.replace(old, new) if index in (None, i) else l for i, l in enumerate(o)]
        cases = [
            (wifi(on=b'PLWIF1 on result=-1 ms=20000\r\n', scans=0)[:2], 'wifi_on'),
            (wifi(on=b'PLWIF1 on result=0 ms=30001\r\n'), 'wifi_on'),
            (wifi()[:2] + [b'PLWIF1 error timeout stage=1 ms=20250\r\n'], 'wifi_timeout'),
            ([b'PLWIF1 start created=1\r\n', b'PLWIF1 error timeout stage=0 ms=30250\r\n'], 'wifi_timeout'),
            (wifi(edit=change(b'created=1', b'created=0')), 'wifi_start'),
            (wifi(edit=change(b'result=0 ms=3100', b'result=-2 ms=3100')), 'wifi_scan'),
            (wifi(edit=change(b'stopped=1', b'stopped=0')), 'wifi_scan'),
            (wifi(edit=change(b'ms=3100', b'ms=10001')), 'wifi_scan'),
            (wifi(edit=change(b'dropped=0', b'dropped=1')), 'wifi_scan'),  # dropped only once 32 are kept
            (wifi(edit=change(b'count=3 ', b'count=33 ')), 'wifi_scan'),
            (wifi(edit=lambda o: o[:3] + o[4:]), 'wifi_scan'),
            (wifi(edit=change(b' i=1 ', b' i=2 ')), 'wifi_networks'),
            (wifi(edit=change(b'rssi=-41', b'rssi=1')), 'wifi_networks'),
            (wifi(edit=change(b'channel=6', b'channel=15')), 'wifi_networks'),
            (wifi(edit=change(b'n=2 result', b'n=3 result')), 'wifi_scan'),
            (wifi(scans=2), 'wifi_sequence'),
            (wifi(edit=lambda o: o + [b'PLADC1 open outputs=1\r\n']), 'wifi_sequence'),
            (wifi(edit=change(b'rssi=-41', b'rssi=-4x')), 'wifi_sequence'),
        ]
        for index, (lines, outcome) in enumerate(cases):
            with self.subTest(index=index, outcome=outcome):
                report = self.audit(capture(self.identity, lines))
                self.assertEqual(report['outcome'], outcome)
        chunks, finished = capture(self.identity)
        early = [(t if b'PLWIF1 start' not in d else 4.0, d) for t, d in chunks]
        self.assertEqual(self.audit((early, finished))['outcome'], 'wifi_sequence')
        chunks.append((finished - 1, startup(self.identity)))
        self.assertEqual(self.audit((chunks, finished))['outcome'], 'startup_repeated_startup')

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        self.policy(None)
        with self.assertRaisesRegex(ValueError, 'Wi-Fi policy required'):
            self.audit(good)
        self.manifest['startup_wifi_policy'] = 'fw07-scan-v1'
        self.image(self.base)
        with self.assertRaisesRegex(ValueError, 'Wi-Fi policy/artifact mismatch'):
            self.audit(good)
        self.policy('fw07-scan-v2')
        with self.assertRaisesRegex(ValueError, 'Wi-Fi policy required'):
            self.audit(good)


if __name__ == '__main__':
    unittest.main()
