from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_supervision', TOOLS / 'audit-firmware-supervision.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
S = M.STARTUP
T0 = datetime(2026, 10, 20, 10, 0, tzinfo=timezone.utc)
POWER = b'PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r'
CLOCK = b'PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n'
OUTPUTS = b'PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n'
ROUTING = b''.join(f'PLIO1 stage={s} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001\r\n'.encode()
                   for s in ('startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22'))
LED = b'PLLED1 begin output_only=PA0,PA2,PA4\r\n' + b''.join(
    b'PLLED1 pin=' + pin + b' level=' + n + b' status=' + status + b'\r\n'
    for pin, status in ((b'PA0', b'output'), (b'PA2', b'output'), (b'PA4', b'output'), (b'PB22', b'pass')) for n in (b'1', b'0', b'1', b'0')) + M.LED_DONE
ARM = b'PLWDG1 arm status=pass vendor=56c1001f count=11 divisor=31 marker=50465735\r\n'
SKIP = b'PLWDG1 skip reset=00100000 marker=50465735\r\n'


def startup(identity, reset=b'00000000'):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Petlibro local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


def snap(kind, raw, ms, irqs, flags=b'1 1 1'):
    ready, healthy, stopped = flags.split()
    return b'PLTMR1 %s raw=%08x ms=%d irqs=%d ready=%s healthy=%s stopped=%s\r\n' % (kind, raw % (1 << 32), ms, irqs, ready, healthy, stopped)


def chunks(identity, hz=32768.0, reset_after=3.9, stall_ticks=65600, fed=b'PLWDG1 fed ms=15000 checkpoints=60 status=pass\r\n', stalled=(1000, 2000, 3000), fault=4, boot2_tail=LED + SKIP):
    """(host seconds, bytes) receive chunks of a realistic two-boot FW05 capture."""
    out = [(0.0, startup(identity)), (5.0, LED)]
    t, raw, ms, irqs = 21.0, 0xfff00000, 20000, 0
    out.append((t, b'PLTMR1 open board=1 events=1 hz=32768 period_ms=10 status=pass\r\n'))
    for n in range(13):
        ticks = 3277 if n == 0 else 163840
        t += ticks / hz
        raw, ms, irqs = raw + ticks, ms + round(ticks * 1000 / 32768), irqs + round(ticks / 328)
        out.append((t, snap(b'sample', raw, ms, irqs)))
    t += 2.1
    out.append((t, snap(b'stall_begin', raw + 30, ms, irqs) + snap(b'stall_end', raw + 30 + stall_ticks, ms + 2002, irqs + round(stall_ticks / 328))))
    out += [(t + 0.01, ARM), (t + 15.3, fed), (t + 15.31, b'PLWDG1 stall_begin ms=99000\r\n')]
    stall = t + 15.31
    out += [(stall + v / 1000, b'PLWDG1 stalled ms=%d fault=%d enabled=1\r\n' % (v, fault)) for v in stalled]
    out += [(stall + reset_after, startup(identity, b'00100000')), (stall + reset_after + 5, boot2_tail)]
    return out


class FirmwareSupervision(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_supervision_policy='fw05-probe-v1')
        self.image(f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                   'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT + b' PLTMR1 open board=')

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def write(self, received):
        rx, events, offset = b''.join(data for _, data in received), [], 0
        for seconds, data in received:
            events.append({'utc': (T0 + timedelta(seconds=seconds)).isoformat(), 'direction': 'rx', 'offset': offset, 'bytes': len(data)})
            offset += len(data)
        (self.p / 'serial-rx.bin').write_bytes(rx)
        (self.p / 'serial-tx.bin').write_bytes(b'')
        (self.p / 'serial-events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        (self.p / 'report.json').write_text(json.dumps({'mode': 'observe', 'status': 'complete', 'start_utc': T0.isoformat(),
                                                        'end_utc': (T0 + timedelta(seconds=offset and received[-1][0] + 10)).isoformat(),
                                                        'serial-rx.bin_sha256': S.digest(rx), 'serial-tx.bin_sha256': S.digest(b'')}))

    def audit(self, received):
        self.write(received)
        return M.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin')

    def startup_audit(self, received):
        self.write(received)
        return S.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin', 1)

    def test_two_boot_capture_passes_with_estimates(self):
        report = self.audit(chunks(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['startups'], 2)
        self.assertEqual(report['timer_samples'], 13)
        self.assertLess(abs(report['systimer_error_ppm']), 100)
        self.assertAlmostEqual(report['tim2_period_ms_estimate'], 10.0, delta=0.02)
        self.assertAlmostEqual(report['watchdog_reset_seconds'], 3.9, delta=0.001)
        self.assertAlmostEqual(report['watchdog_clock_hz_estimate'], 131040 / 3.9, delta=1)
        self.assertEqual((report['boot1']['outcome'], report['boot2']['outcome']), ('pass', 'pass'))
        self.assertEqual((report['boot2_reset_flags'], report['boot2_skip_reset']), ('00100000', '00100000'))
        self.assertIn('USB latency', report['limit'])
        # The single-boot auditor keeps its semantics: two startups are a repeated startup.
        self.assertEqual(self.startup_audit(chunks(self.identity))['outcome'], 'repeated_startup')

    def test_failures_have_distinct_outcomes(self):
        good = chunks(self.identity)

        def edit(old, new, data=None):
            return [(t, d.replace(old, new)) for t, d in data or good]
        no_reset = good[:-2] + [(good[-3][0] + 0.5, b'PLWDG1 error no_reset ms=8000\r\n')] + good[-2:]
        cases = [
            (good[:-2], 'wrong_boot_count'),
            (good + [(t + 20, d) for t, d in good[-2:]], 'wrong_boot_count'),
            (edit(b'ms=30100 ', b'ms=30110 '), 'timer_consistency'),
            (edit(b'irqs=1510 ', b'irqs=1560 '), 'timer_consistency'),
            (chunks(self.identity, hz=32768 / 1.03), 'systimer_rate'),
            (chunks(self.identity, stall_ticks=60000), 'stall_failed'),
            (edit(b'arm status=pass', b'arm status=fail'), 'watchdog_arm'),
            (edit(b'vendor=56c1001f', b'vendor=16c1001f'), 'watchdog_arm'),
            (edit(b'divisor=31 marker=50465735', b'divisor=31 marker=50465736'), 'watchdog_arm'),
            (edit(b'count=11 divisor=31', b'count=11 divisor=30'), 'watchdog_arm'),
            (chunks(self.identity, fed=b'PLWDG1 fed ms=9000 checkpoints=36 status=pass\r\n'), 'watchdog_feed'),
            (chunks(self.identity, fault=2), 'watchdog_stall'),
            (no_reset, 'watchdog_no_reset'),
            (edit(b'PLTMR1 sample', b'PLTMR1 error clock\r\n', good[:5]) + good[5:], 'timer_clock'),
            (chunks(self.identity, reset_after=1.5, stalled=(1000,)), 'reset_timing'),
            (chunks(self.identity, reset_after=9.0), 'reset_timing'),
            (edit(b'stalled ms=3000 ', b'stalled ms=4500 '), 'reset_timing'),
            (chunks(self.identity, boot2_tail=LED), 'boot2_skip'),
            (chunks(self.identity, boot2_tail=LED + SKIP.replace(b'marker=50465735', b'marker=00000000')), 'boot2_skip'),
            (chunks(self.identity, boot2_tail=SKIP + LED), 'boot2_skip'),
            (chunks(self.identity, boot2_tail=LED + SKIP + SKIP), 'boot2_skip'),
            (chunks(self.identity, boot2_tail=LED + SKIP + ARM), 'boot2_skip'),
            (edit(b'PLTMR1 open board=1 events=1', b'PLTMR1 open board=1 events=0'), 'timer_open'),
            ([(t, d) for t, d in good if b'PLTMR1 sample' not in d or t < 30], 'timer_samples'),
            (good[:6] + edit(b'healthy=1', b'healthy=0', good[6:7]) + good[7:], 'timer_samples'),
            (edit(b'PLTMR1 open', b'PLTMR1 opened'), 'supervision_sequence'),
            ([good[0], (good[1][0], good[2][1]), (good[2][0], good[1][1])] + good[3:], 'supervision_sequence'),
            (edit(M.LED_DONE, b'', good[:2]) + good[2:], 'boot1_led_incomplete'),
            (good[:-1] + [(good[-1][0], b'PLLED1 begin output_only=PA0,PA2,PA4\r\n' + SKIP)], 'boot2_led_incomplete'),
        ]
        for received, outcome in cases:
            with self.subTest(outcome=outcome):
                report = self.audit(received)
                self.assertEqual(report['outcome'], outcome)
                self.assertNotIn('PLTMR1', json.dumps(report))

    def test_policy_binding_in_both_auditors(self):
        good = chunks(self.identity)
        self.manifest.pop('startup_supervision_policy')
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, 'Supervision policy'):
            self.audit(good)
        with self.assertRaisesRegex(ValueError, 'Supervision policy/artifact mismatch'):
            self.startup_audit(good)
        self.manifest['startup_supervision_policy'] = 'fw05-probe-v2'
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, 'Unknown startup supervision'):
            self.startup_audit(good)
        self.manifest['startup_supervision_policy'] = 'fw05-probe-v1'
        image = (self.p / 'image.bin').read_bytes().replace(b' PLTMR1 open board=', b'')
        self.image(image)
        for call in (self.audit, self.startup_audit):
            with self.assertRaisesRegex(ValueError, 'Supervision'):
                call(good)

    def test_legacy_manifest_audits_single_boot_as_before(self):
        self.manifest.pop('startup_supervision_policy')
        self.image((self.p / 'image.bin').read_bytes().replace(b' PLTMR1 open board=', b''))
        single = [(0.0, startup(self.identity)), (5.0, LED)]
        report = self.startup_audit(single)
        self.assertEqual(report['outcome'], 'pass')
        self.assertNotIn('startup_supervision_policy', report)
        with self.assertRaisesRegex(ValueError, 'Supervision policy required'):
            self.audit(single)


if __name__ == '__main__':
    unittest.main()
