from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_adc', TOOLS / 'audit-firmware-adc.py')
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
ADC_CLOCK = b'PLADC1 clock function_before=000115f7 clock_before=00031457 function_after=000115f7 clock_after=00031557 status=pass\r\n'
OPEN = b'PLADC1 open outputs=1 efuse=ffffffff offset=2480 gain=12050 default_offset=1 default_gain=1 status=pass\r\n'


def startup(identity, reset=b'00000000'):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Wellfed local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


def sample(n, current=(290, 310, 300), battery=(2590, 2610, 2600), ticks=420, dc=1, stopped=1, scaled=None):
    def channel(low, high, mean):
        return b'%d,%d,%d,%d' % (low, high, mean, M.scaled(mean, 2480, 12050))
    line = b'PLADC1 sample n=%d ticks=%d food=0 outlet=1 dc=%d stopped=%d current=%s battery=%s\r\n' % (
        n, ticks, dc, stopped, channel(*current), channel(*battery))
    return line if scaled is None else line.replace(b',%d battery' % M.scaled(current[2], 2480, 12050), b',%d battery' % scaled)


def capture(identity, samples=40, period=1.0, clock=ADC_CLOCK, opened=OPEN, line=sample):
    out = [(0.0, startup(identity)), (5.0, LED), (5.05, clock + opened)]
    out += [(6.05 + n * period, line(n)) for n in range(samples)]
    return out, 6.05 + samples * period + 2


class FirmwareAdc(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy=None,
                             startup_adc_policy='fw14-adc-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT)
        self.image(self.base + b' ' + S.ADC_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def policy(self, policy):
        self.manifest['startup_adc_policy'] = policy
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

    def test_read_only_capture_passes_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['startup']['outcome'], 'pass')
        self.assertFalse(report['adc_enabled_before'])
        self.assertEqual(report['adc_calibration'], {'offset': 2480, 'gain': 12050, 'default_offset': 1, 'default_gain': 1})
        self.assertEqual(report['adc_samples'], 40)
        self.assertEqual(report['adc_sample_seconds'], 1.0)
        self.assertEqual(report['adc_ticks_max'], 420)
        self.assertEqual((report['adc_food_levels'], report['adc_outlet_levels'], report['adc_dc_levels']), ([0], [1], [1]))
        self.assertEqual(report['adc_current'], {'min': 290, 'max': 310, 'mean_min': 300, 'mean_max': 300, 'spread_max': 20,
                                                 'scaled_min': 43, 'scaled_max': 43})
        self.assertEqual(report['adc_battery']['scaled_max'], (26000 - 2480) * 1000 // 12050)
        self.assertIn('not established', report['limit'])
        self.assertNotIn('PLADC1', json.dumps(report))
        # An ADC already enabled by startup is preserved and still passes.
        on = ADC_CLOCK.replace(b'clock_before=00031457', b'clock_before=00031557').replace(b'function_before=000115f7', b'function_before=000115f7')
        report = self.audit(capture(self.identity, clock=on))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertTrue(report['adc_enabled_before'])
        # Real efuse calibration decodes little endian.
        opened = b'PLADC1 open outputs=1 efuse=a8090f2f offset=2472 gain=12047 default_offset=0 default_gain=0 status=pass\r\n'
        line = lambda n: sample(n).replace(b',300,43 ', b',300,%d ' % M.scaled(300, 2472, 12047)).replace(  # noqa: E731
            b',2600,%d\r' % M.scaled(2600, 2480, 12050), b',2600,%d\r' % M.scaled(2600, 2472, 12047))
        self.assertEqual(self.audit(capture(self.identity, opened=opened, line=line))['outcome'], 'pass')

    def test_failures_have_distinct_outcomes(self):
        def lines(change):
            return lambda n: change(n, sample(n))
        cases = [
            (capture(self.identity, clock=ADC_CLOCK.replace(b'status=pass', b'status=fail')), 'adc_clock'),
            (capture(self.identity, clock=ADC_CLOCK.replace(b'clock_after=00031557', b'clock_after=00031457')), 'adc_clock'),
            (capture(self.identity, clock=ADC_CLOCK.replace(b'function_after=000115f7', b'function_after=000117f7')), 'adc_clock'),
            (capture(self.identity, opened=OPEN.replace(b'outputs=1', b'outputs=0')), 'adc_open'),
            (capture(self.identity, opened=OPEN.replace(b'status=pass', b'status=fail')), 'adc_open'),
            (capture(self.identity, opened=OPEN.replace(b'efuse=ffffffff', b'efuse=none')), 'adc_open'),
            (capture(self.identity, opened=OPEN.replace(b'gain=12050', b'gain=12051')), 'adc_open'),
            (capture(self.identity, opened=OPEN.replace(b'default_gain=1', b'default_gain=0')), 'adc_open'),
            (capture(self.identity, samples=29), 'adc_samples'),
            (capture(self.identity, samples=0), 'adc_samples'),
            (capture(self.identity, line=lambda n: sample(n + (n > 5))), 'adc_samples'),
            (capture(self.identity, line=lines(lambda n, s: sample(n, stopped=0) if n == 7 else s)), 'adc_samples'),
            (capture(self.identity, line=lines(lambda n, s: sample(n, ticks=0) if n == 7 else s)), 'adc_samples'),
            (capture(self.identity, line=lines(lambda n, s: sample(n, ticks=3278) if n == 7 else s)), 'adc_samples'),
            (capture(self.identity, line=lines(lambda n, s: sample(n, scaled=44) if n == 7 else s)), 'adc_values'),
            (capture(self.identity, line=lines(lambda n, s: sample(n, current=(290, 299, 300)) if n == 7 else s)), 'adc_values'),
            (capture(self.identity, line=lines(lambda n, s: sample(n, battery=(2590, 4096, 2600)) if n == 7 else s)), 'adc_values'),
            (capture(self.identity, period=0.5), 'adc_timing'),
            (capture(self.identity, period=2.0), 'adc_timing'),
            (capture(self.identity, line=lines(lambda n, s: b'PLADC1 error n=7 ticks=3277 stopped=1\r\n' if n == 7 else s)), 'adc_sequence'),
            (capture(self.identity, line=lines(lambda n, s: s + b'PLSET1 start marker=00000000 watchdog=pass installed=1\r\n' if n == 3 else s)),
             'adc_sequence'),
            (capture(self.identity, line=lines(lambda n, s: s.replace(b'food=0', b'food=2') if n == 3 else s)), 'adc_sequence'),
            (capture(self.identity, opened=b''), 'adc_sequence'),
            (capture(self.identity, line=lines(lambda n, s: s + OPEN if n == 3 else s)), 'adc_sequence'),
        ]
        for index, (received, outcome) in enumerate(cases):
            with self.subTest(index=index, outcome=outcome):
                report = self.audit(received)
                self.assertEqual(report['outcome'], outcome)
                self.assertNotIn('PLADC1', json.dumps(report))
        # Records before the LED probe completes, or a second startup.
        chunks, finished = capture(self.identity)
        early = [(t if b'PLADC1 clock' not in d else 4.0, d) for t, d in chunks]
        self.assertEqual(self.audit((early, finished))['outcome'], 'adc_sequence')
        chunks.append((finished - 1, startup(self.identity)))
        self.assertEqual(self.audit((chunks, finished))['outcome'], 'startup_repeated_startup')

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        self.policy(None)
        with self.assertRaisesRegex(ValueError, 'ADC policy required'):
            self.audit(good)
        self.manifest['startup_adc_policy'] = 'fw14-adc-v1'
        self.image(self.base)
        with self.assertRaisesRegex(ValueError, 'ADC policy/artifact mismatch'):
            self.audit(good)
        self.policy('fw14-adc-v2')
        with self.assertRaisesRegex(ValueError, 'ADC policy required'):
            self.audit(good)
        self.manifest['startup_adc_policy'] = 'fw14-adc-v1'
        self.manifest['startup_led_policy'] = None
        self.image(self.base.replace(b'PLLED1 begin output_only= highs= ', b'') + b' ' + S.ADC_MARK)
        with self.assertRaisesRegex(ValueError, 'ADC probe requires the LED probe policy'):
            self.audit(good)


if __name__ == '__main__':
    unittest.main()
