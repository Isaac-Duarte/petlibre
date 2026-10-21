from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_settings', TOOLS / 'audit-firmware-settings.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
S = M.STARTUP
T0 = datetime(2026, 10, 21, 10, 0, tzinfo=timezone.utc)
POWER = b'PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r'
CLOCK = b'PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n'
OUTPUTS = b'PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n'
ROUTING = b''.join(f'PLIO1 stage={s} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001\r\n'.encode()
                   for s in ('startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22'))
LED_BEGIN = b'PLLED1 begin output_only=PA0,PA2,PA4\r\n'
LED_REST = b''.join(
    b'PLLED1 pin=' + pin + b' level=' + n + b' status=' + status + b'\r\n'
    for pin, status in ((b'PA0', b'output'), (b'PA2', b'output'), (b'PA4', b'output'), (b'PB22', b'pass')) for n in (b'1', b'0', b'1', b'0')) + b'PLLED1 done low=all\r\n'
START = b'PLSET1 start marker=00000000 watchdog=pass installed=1\r\n'
SAVE_SECONDS = 0.125  # ~8 saves/s
OFF_SECONDS = 3.0
# (open status, open generation, completed saves (300 ends), cut fragment); None cuts the boot during its LED probe.
BOOTS = (
    (1, 0, 130, b'PLSET1 batch sa'),  # last report 125; the save in progress at the cut completes as 131
    None,
    (2, 131, 60, b''),  # cut cleanly between records; last report 181
    (0, 192, 300, None),  # completes, then power is cut while idle
    (0, 492, 40, b'PLSE'),  # last report 517
    (0, 532, 17, b'PLSET1 ba'),  # the recording ends mid-line
)


def startup(identity, reset):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Wellfed local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


def opened(status, generation, consistent=1):
    return b'PLSET1 open status=%d generation=%d volume=%d lights=%d slot=%d consistent=%d\r\n' % (
        status, generation, generation % 101, generation % 2, generation % 2, consistent)


def batch(saves, generation, ticks):
    return b'PLSET1 batch saves=%d generation=%d ticks_max=%d\r\n' % (saves, generation, ticks)


def end(generation, leases=1503, frozen=None, saves=300, phase=1, fault=0):
    return b'PLSET1 end saves=%d generation=%d leases=%d frozen=%d phase=%d fault=%d\r\n' % (
        saves, generation, leases, leases if frozen is None else frozen, phase, fault)


def capture(identity, boots=BOOTS, resets=None, off=OFF_SECONDS, tail=20.0):
    """(host seconds, bytes, boot, label) receive chunks of a realistic power-cut settings capture."""
    out, t0 = [], 0.0
    for number, spec in enumerate(boots, 1):
        reset = (resets or {}).get(number, b'00000000')
        out += [(t0, startup(identity, reset), number, 'startup'), (t0 + 3.0, LED_BEGIN, number, 'led')]
        if spec is None:
            out.append((t0 + 6.5, LED_REST[:160], number, 'fragment'))
            t0 += 6.5 + off
            continue
        status, generation, saves, fragment = spec
        out.append((t0 + 17.0, LED_REST, number, 'led'))
        t = t0 + 17.2
        out.append((t, b'PLFLS1 hello authority=000000010000a1b2 count=1\r\n', number, 'hello'))
        t += 0.3
        out.append((t, b'PLFLS1 ready peer=000000010000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n', number, 'ready'))
        t += 0.05
        out.append((t, START, number, 'start'))
        t += 0.06
        out.append((t, opened(status, generation), number, 'open'))
        for n in range(1, min(saves, 299) + 1):
            if n % 25 == 0:
                out.append((t + n * SAVE_SECONDS, batch(n, generation + n, 3900 + n), number, f'batch{n}'))
        t += saves * SAVE_SECONDS
        if saves == 300:
            out.append((t, end(generation + 300), number, 'end'))
        if fragment is None:
            t += 1.0  # idle, then cut
        elif number % 2:
            # The fragment shares a receive chunk with the last complete record.
            last = len(out) - 1
            out[last] = (out[last][0], out[last][1] + fragment, number, out[last][3])
        elif fragment:
            out.append((t, fragment, number, 'fragment'))
        t0 = t + off
    return out, t0 - off + tail


class FirmwareSettings(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy='fw15-settings-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT + b' ' + S.FLASH_MARK)
        self.image(self.base + b' ' + S.SETTINGS_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def policy(self, policy):
        self.manifest['startup_flash_handshake_policy'] = policy
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def write(self, received):
        chunks, finished = received
        chunks = sorted(chunks, key=lambda c: c[0])
        rx, events, offset = b''.join(c[1] for c in chunks), [], 0
        for c in chunks:
            events.append({'utc': (T0 + timedelta(seconds=c[0])).isoformat(), 'direction': 'rx', 'offset': offset, 'bytes': len(c[1])})
            offset += len(c[1])
        (self.p / 'serial-rx.bin').write_bytes(rx)
        (self.p / 'serial-tx.bin').write_bytes(b'')
        (self.p / 'serial-events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        (self.p / 'report.json').write_text(json.dumps({'mode': 'observe', 'status': 'complete', 'start_utc': T0.isoformat(),
                                                        'end_utc': (T0 + timedelta(seconds=finished)).isoformat(),
                                                        'serial-rx.bin_sha256': S.digest(rx), 'serial-tx.bin_sha256': S.digest(b'')}))

    def audit(self, received):
        self.write(received)
        return M.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin')

    def startup_audit(self, received):
        self.write(received)
        return S.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin', 1)

    def boots(self, changes):
        boots = list(BOOTS)
        for index, spec in changes.items():
            boots[index] = spec
        return capture(self.identity, boots)

    def test_power_cut_capture_passes_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['startups'], 6)
        self.assertEqual(report['boot_reset_flags'], ['00000000'] * 6)
        self.assertEqual(report['boot_opened'], [True, False, True, True, True, True])
        self.assertIsNone(report['boot_startups'][1])
        self.assertTrue(all(c['outcome'] == 'pass' and c['flash_handshake_verified'] for c in report['boot_startups'] if c))
        self.assertEqual([o and (o['status'], o['generation'], o['consistent']) for o in report['boot_opens']],
                         [(1, 0, 1), None, (2, 131, 1), (0, 192, 1), (0, 492, 1), (0, 532, 1)])
        self.assertEqual(report['boot_last_generations'], [125, None, 181, 492, 517, 532])
        self.assertEqual(report['settings_recovery_deltas'], [6, 11, 0, 15])
        self.assertEqual(report['settings_ticks_max'], 3900 + 275)
        self.assertEqual(report['settings_save_ms_max'], round(4175 * 1000 / 32768, 1))
        self.assertEqual(report['settings_cuts'], 3)
        self.assertEqual(report['boot_ends'], [None, None, dict(saves=300, generation=492, leases=1503, frozen=1503, phase=1, fault=0),
                                               None, None])
        self.assertIn('USB latency', report['limit'])
        self.assertIn('not measured', report['limit'])
        self.assertIn('retention', report['limit'])
        self.assertNotIn('PLSET1', json.dumps(report))
        # The single-boot auditor keeps its semantics: several startups are a repeated startup.
        self.assertEqual(self.startup_audit(capture(self.identity))['outcome'], 'repeated_startup')

    def test_bounds_and_minimal_captures_pass(self):
        cases = [
            # Three opened boots, two of them cut after their open, a recovery at both bounds, the save-time bound.
            capture(self.identity, ((1, 0, 30, b'PLSET1 open'), (0, 25 + 25, 3, b''), (0, 53, 0, None))),
            capture(self.identity, ((0, 7, 24, b'PLSET1 batch saves=25 gen'), (2, 7, 0, b''), (0, 7, 2, b''))),
            capture(self.identity, ((1, 0, 0, b''), (0, 0, 300, None), (0, 300, 12, b'P'), (0, 300, 300, None))),
        ]
        for received in cases:
            with self.subTest(received=len(received[0])):
                self.assertEqual(self.audit(received)['outcome'], 'pass', self.audit(received))
        chunks, finished = capture(self.identity)
        chunks = [(t, d.replace(b'ticks_max=4175', b'ticks_max=26214'), b, k) for t, d, b, k in chunks]
        chunks.append((next(c[0] for c in chunks if c[3] == 'hello') + 0.1, b'PLFLS1 foreign\r\n', 1, 'foreign'))
        report = self.audit((chunks, finished))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['settings_save_ms_max'], 800.0)
        # A boot cut before its PLSET1 open, even mid start record, contributes nothing.
        chunks, finished = capture(self.identity)
        boot2 = next(c for c in chunks if c[2] == 2 and c[3] == 'startup')
        chunks = [c if c is not boot2 else (c[0], c[1][:c[1].index(b'PLFW1 start') + 30], 2, 'startup')
                  for c in chunks if c[2] != 2 or c[3] == 'startup']
        report = self.audit((chunks, finished))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['boot_reset_flags'][1], None)
        # A short power cut still splits boots.
        self.assertEqual(self.audit(capture(self.identity, off=M.POWER_OFF_SECONDS + 0.1))['outcome'], 'pass')

    def test_single_boot_settings_handshake_under_startup_auditor(self):
        chunks, finished = capture(self.identity, ((0, 192, 300, None),))
        report = self.startup_audit((chunks, finished))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertTrue(report['flash_handshake_verified'])
        ready = next(c for c in chunks if c[3] == 'ready')
        start = next(c for c in chunks if c[3] == 'start')
        stress = [c for c in chunks if c[3] not in ('start', 'open', 'end') and not c[3].startswith('batch')]
        cases = [
            (stress, 'flash_handshake_missing'),
            ([c for c in chunks if c[3] != 'hello'], 'flash_handshake_missing'),
            ([c if c[3] != 'start' else (ready[0] - 0.01, c[1], 1, 'start') for c in chunks], 'flash_handshake_order'),
            ([c if c[3] != 'ready' else (c[0] + 2.5, c[1], 1, 'ready') for c in stress]
             + [(ready[0] + 2.51, start[1], 1, 'start')], 'flash_handshake_timing'),
            ([(t, d.replace(b'reply=0', b'reply=1'), b, k) for t, d, b, k in chunks], 'flash_handshake_failed'),
            (chunks + [(start[0] + 0.01, b'PLFLS1 error clock\r\n', 1, 'x')], 'flash_handshake_failed'),
            (chunks + [(start[0] + 0.01, b'PLFLS1 done phase=8 fault=7 blocked=1\r\n', 1, 'x')], 'flash_handshake_malformed'),
        ] + [(chunks + [(start[0] + 0.01, line, 1, 'x')], 'flash_handshake_malformed') for line in (
            b'PLPRK1 start mode=2 marker=00000002 watchdog=pass\r\n',
            b'PLRDS1 start marker=00000000 watchdog=pass installed=1\r\n',
            b'PLWRS1 start marker=00000000 watchdog=pass installed=1\r\n',
            b'PLJNL1 start mode=0 marker=00000000 watchdog=pass installed=1\r\n')]
        for received, outcome in cases:
            with self.subTest(outcome=outcome):
                result = self.startup_audit((received, finished))
                self.assertEqual(result['outcome'], outcome)
                self.assertFalse(result['flash_handshake_verified'])

    def test_failures_have_distinct_outcomes(self):
        good, finished = capture(self.identity)

        def at(label, boot):
            return next(c for c in good if c[2] == boot and c[3] == label)

        def edit(old, new, boot=None, label=None):
            return [(t, d.replace(old, new) if (boot in (None, b) and label in (None, k)) else d, b, k) for t, d, b, k in good], finished

        def drop(boot, label):
            return [c for c in good if (c[2], c[3]) != (boot, label)], finished

        def add(boot, label, data, delay=0.01):
            return good + [(at(label, boot)[0] + delay, data, boot, 'extra')], finished
        shift = at('startup', 2)[0] - max(c[0] for c in good if c[2] == 1) - 0.1
        cases = [
            (add(3, 'batch25', b'PLSET1 error save generation=156 status=5\r\n'), 'settings_sequence'),
            (add(3, 'batch25', b'PLSET1 skip marker=00000001\r\n'), 'settings_sequence'),
            (add(2, 'led', b'PLFLS1 error clock\r\n'), 'settings_sequence'),
            (add(2, 'led', b'PLSET1 error save generation=1 status=5\r\n'), 'settings_sequence'),
            (add(2, 'led', b'PLJNL1 start mode=0 marker=00000000 watchdog=pass installed=1\r\n'), 'settings_sequence'),
            (edit(b'saves=50 generation', b'saves=5x generation', 3), 'settings_sequence'),
            (drop(3, 'start'), 'settings_sequence'),
            (add(3, 'open', opened(2, 131)), 'settings_sequence'),
            (add(3, 'batch25', START), 'settings_sequence'),
            (add(4, 'end', b'PLSET1 batch saves=301'), 'settings_sequence'),
            # Without a power-off gap after boot 1's last record, that record would open boot 2.
            (([c if c[2] == 1 else (c[0] - shift, *c[1:]) for c in good], finished), 'settings_sequence'),
            (capture(self.identity, resets={2: b'00100000'}), 'wrong_reset'),
            (capture(self.identity, resets={4: b'00100000'}), 'wrong_reset'),
            (edit(b'PLLED1 done low=all\r\n', b'', 3), 'boot3_led_incomplete'),
            (edit(b'reply=0', b'reply=1', 4), 'boot4_flash_handshake_failed'),
            (drop(6, 'ready'), 'boot6_flash_handshake_missing'),
            (([c if (c[2], c[3]) != (5, 'start') else (at('ready', 5)[0] - 0.01, c[1], 5, 'start') for c in good], finished),
             'boot5_flash_handshake_order'),
            (capture(self.identity, BOOTS[:3]), 'settings_boots'),
            (capture(self.identity, (None, None)), 'settings_boots'),
            (edit(b'consistent=1', b'consistent=0', 4), 'settings_open'),
            (edit(b'status=2', b'status=1', 3), 'settings_open'),
            (edit(b'status=0', b'status=3', 5), 'settings_open'),
            (edit(b'status=1', b'status=0', 1), 'pass'),
            (self.boots({0: (1, 5, 130, b'')}), 'settings_open'),
            (edit(b'marker=00000000', b'marker=00000001', 4, 'start'), 'settings_open'),
            (edit(b'watchdog=pass', b'watchdog=fail', 5), 'settings_open'),
            (edit(b'installed=1', b'installed=0', 6), 'settings_open'),
            (self.boots({2: (2, 124, 60, b'')}), 'settings_recovery'),
            (self.boots({2: (2, 151, 60, b'')}), 'settings_recovery'),
            (self.boots({3: (0, 207, 300, None)}), 'settings_recovery'),
            (self.boots({4: (0, 491, 40, b'PLSE')}), 'settings_recovery'),
            (edit(b'saves=50 generation=181', b'saves=75 generation=181', 3), 'settings_batches'),
            (drop(4, 'batch100'), 'settings_batches'),
            (edit(b'generation=181', b'generation=182', 3), 'settings_batches'),
            (edit(b'ticks_max=3925', b'ticks_max=0', 1), 'settings_batches'),
            (edit(b'ticks_max=3925', b'ticks_max=26215', 4), 'settings_batches'),
            (add(4, 'batch275', batch(300, 492, 4200), 0.1), 'settings_batches'),
            (edit(end(492), end(491), 4), 'settings_end'),
            (edit(end(492), end(492, saves=299), 4), 'settings_end'),
            (edit(end(492), end(492, phase=8), 4), 'settings_end'),
            (edit(end(492), end(492, fault=7), 4), 'settings_end'),
            (edit(end(492), end(492, frozen=1502), 4), 'settings_end'),
            (edit(end(492), end(492, leases=0), 4), 'settings_end'),
            ((good[:-1] + [(good[-1][0], end(532 + 17), 6, 'end')], finished), 'settings_end'),
            (capture(self.identity, ((1, 0, 130, b'PLS'), (2, 131, 300, None), (0, 431, 300, None), (0, 731, 4, b'PL'))), 'settings_cuts'),
            # The last boot ends with the recording, not a power cut.
            (capture(self.identity, ((1, 0, 30, b''), (0, 30, 300, None), (0, 330, 3, b''))), 'settings_cuts'),
        ]
        for index, (received, outcome) in enumerate(cases):
            with self.subTest(index=index, outcome=outcome):
                report = self.audit(received)
                self.assertEqual(report['outcome'], outcome)
                self.assertNotIn('PLSET1', json.dumps(report))

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        # A settings image cannot be audited under another policy, or without a policy.
        for other in ('fw06-hello-v1', 'fw06-park-v1', 'fw15-journal-v1'):
            self.policy(other)
            with self.assertRaisesRegex(ValueError, 'Settings policy required'):
                self.audit(good)
        self.policy('fw06-hello-v1')
        with self.assertRaisesRegex(ValueError, 'Flash settings policy/artifact mismatch'):
            self.startup_audit(good)
        self.policy(None)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.startup_audit(good)
        self.policy('fw15-settings-v2')
        with self.assertRaisesRegex(ValueError, 'Unknown flash handshake policy'):
            self.startup_audit(good)
        # A hello-only, park or journal image cannot be audited under the settings policy.
        self.policy('fw15-settings-v1')
        self.image(self.base)
        for call in (self.audit, self.startup_audit):
            with self.assertRaisesRegex(ValueError, 'Settings|settings'):
                call(good)
        self.image(self.base + b' ' + S.PARK_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash park policy/artifact mismatch'):
            self.startup_audit(good)
        self.image(self.base + b' ' + S.JOURNAL_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash journal policy/artifact mismatch'):
            self.startup_audit(good)
        # The settings policy still requires the HELLO record format.
        self.image(self.base.replace(S.FLASH_MARK, b'') + S.SETTINGS_MARK)
        for call in (self.audit, self.startup_audit):
            with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
                call(good)

    def test_other_policies_refuse_settings_records_and_keep_their_outcomes(self):
        # A journal image carrying the settings start format is refused under the journal policy.
        self.policy('fw15-journal-v1')
        self.image(self.base + b' ' + S.JOURNAL_MARK + b' ' + S.SETTINGS_MARK)
        boot = capture(self.identity, ((0, 192, 300, None),))
        with self.assertRaisesRegex(ValueError, 'Flash settings policy/artifact mismatch'):
            self.startup_audit(boot)
        # A journal image without the settings format keeps the journal handshake semantics.
        self.image(self.base + b' ' + S.JOURNAL_MARK)
        chunks, finished = boot
        chunks = [(t, d.replace(START, b'PLJNL1 start mode=2 marker=00000002 watchdog=pass installed=1\r\n'), b, k)
                  for t, d, b, k in chunks if k not in ('open', 'end') and not k.startswith('batch')]
        self.assertEqual(self.startup_audit((chunks, finished))['outcome'], 'pass')


if __name__ == '__main__':
    unittest.main()
