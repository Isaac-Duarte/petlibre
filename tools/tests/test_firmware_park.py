from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_park', TOOLS / 'audit/audit-firmware-park.py')
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
TIMEOUT = b'PLPRK1 inject mode=1 lease=101 begin=1 peer=504b0081 ms=705\r\n'


def startup(identity, reset):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Petlibre local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


def batch(leases, park=310, release=290, frozen=None, alive=None):
    return b'PLPRK1 batch leases=%d park_max=%d release_max=%d frozen=%d alive=%d\r\n' % (
        leases, park, release, leases if frozen is None else frozen, leases if alive is None else alive)


def capture(identity, reset1=3.5, reset2=3.6, tail=30.0, counts=(1, 2, 3), peers=(1, 2, 3)):
    """(host seconds, bytes, boot, label) receive chunks of a realistic three-boot FW06 park capture."""
    out, t0 = [], 0.0
    for boot, (count, peer) in enumerate(zip(counts, peers), 1):
        reset = b'00000000' if boot == 1 else b'00100000'
        out += [(t0, startup(identity, reset), boot, 'startup'), (t0 + 3.0, LED_BEGIN, boot, 'led'), (t0 + 17.0, LED_REST, boot, 'led')]
        t = t0 + 17.2
        out.append((t, b'PLFLS1 hello authority=%08x0000a1b2 count=%d\r\n' % (count, count), boot, 'hello'))
        t += 0.3
        out.append((t, b'PLFLS1 ready peer=%08x0000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n' % peer, boot, 'ready'))
        t += 0.05
        out.append((t, b'PLPRK1 start mode=%d marker=%08x watchdog=pass\r\n' % (boot - 1, boot - 1), boot, 'start'))
        for leases in M.LEASES:
            t += 0.4
            out.append((t, batch(leases), boot, 'batch%d' % leases))
        t += 0.2
        if boot == 1:
            out.append((t, b'PLPRK1 inject mode=0 lease=101 begin=0 peer=00000000 ms=0\r\n', boot, 'inject'))
            t0 = t + reset1
        elif boot == 2:
            out.append((t, b'PLPRK1 inject mode=1 lease=101 begin=1 peer=504b0001 ms=0\r\n', boot, 'inject'))
            out.append((t + 0.71, TIMEOUT, boot, 'release'))
            t0 = t + reset2
        else:
            out.append((t, b'PLPRK1 done leases=100 marker=00000002\r\n', boot, 'done'))
            end = t + tail
    return out, end


class FirmwarePark(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy='fw06-park-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT + b' ' + S.FLASH_MARK)
        self.image(self.base + b' ' + S.PARK_MARK)

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
        chunks, end = received
        chunks = sorted(chunks, key=lambda c: c[0])
        rx, events, offset = b''.join(c[1] for c in chunks), [], 0
        for c in chunks:
            events.append({'utc': (T0 + timedelta(seconds=c[0])).isoformat(), 'direction': 'rx', 'offset': offset, 'bytes': len(c[1])})
            offset += len(c[1])
        (self.p / 'serial-rx.bin').write_bytes(rx)
        (self.p / 'serial-tx.bin').write_bytes(b'')
        (self.p / 'serial-events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        (self.p / 'report.json').write_text(json.dumps({'mode': 'observe', 'status': 'complete', 'start_utc': T0.isoformat(),
                                                        'end_utc': (T0 + timedelta(seconds=end)).isoformat(),
                                                        'serial-rx.bin_sha256': S.digest(rx), 'serial-tx.bin_sha256': S.digest(b'')}))

    def audit(self, received):
        self.write(received)
        return M.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin')

    def startup_audit(self, received):
        self.write(received)
        return S.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin', 1)

    def test_reset_timed_from_next_boot_chunk_when_split_shares_a_chunk(self):
        # Records start with '\r', so the old boot's last chunk can end with the
        # next record's leading byte: the split then falls inside that chunk.
        chunks, end = capture(self.identity)
        chunks = [(t, d + b'\r' if k in ('inject', 'release') else d, b, k) for t, d, b, k in chunks]
        result = self.audit((chunks, end))
        self.assertEqual(result['outcome'], 'pass')
        self.assertAlmostEqual(result['inject_ack_reset_seconds'], 3.5, places=3)
        self.assertAlmostEqual(result['inject_release_reset_seconds'], 3.6, places=3)

    def test_three_boot_capture_passes_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['startups'], 3)
        self.assertEqual([report[f'boot{n}']['outcome'] for n in (1, 2, 3)], ['pass'] * 3)
        self.assertTrue(all(report[f'boot{n}']['flash_handshake_verified'] for n in (1, 2, 3)))
        self.assertEqual(report['boot_reset_flags'], ['00000000', '00100000', '00100000'])
        self.assertEqual(report['boot_hello_counts'], [1, 2, 3])
        self.assertEqual(report['boot_peer_session_high_words'], [1, 2, 3])
        self.assertEqual(report['boot_frozen'], [list(M.LEASES)] * 3)
        self.assertEqual(report['boot_alive'], [list(M.LEASES)] * 3)
        self.assertEqual(report['boot_park_max'], [[310] * 4] * 3)
        self.assertEqual(report['boot_release_max'], [[290] * 4] * 3)
        self.assertAlmostEqual(report['inject_ack_reset_seconds'], 3.5, delta=0.001)
        self.assertAlmostEqual(report['inject_release_reset_seconds'], 3.6, delta=0.001)
        self.assertEqual(report['inject_release_timeout_ms'], 705)
        self.assertAlmostEqual(report['clean_tail_seconds'], 30.0, delta=0.001)
        self.assertIn('USB latency', report['limit'])
        self.assertIn('flash exclusion', report['limit'])
        # The single-boot auditor keeps its semantics: three startups are a repeated startup.
        self.assertEqual(self.startup_audit(capture(self.identity))['outcome'], 'repeated_startup')
        # A one-foreign-line handshake and alive one below leases still pass.
        chunks, end = capture(self.identity)
        chunks = [(t, d.replace(b'alive=50', b'alive=49'), b, k) for t, d, b, k in chunks]
        chunks.append((chunks[3][0] + 0.1, b'PLFLS1 foreign\r\n', 1, 'foreign'))
        self.assertEqual(self.audit((chunks, end))['outcome'], 'pass')

    def test_single_boot_park_handshake_under_startup_auditor(self):
        chunks, end = capture(self.identity)
        boot3 = [c for c in chunks if c[2] == 3]
        report = self.startup_audit((boot3, end))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertTrue(report['flash_handshake_verified'])
        self.assertEqual((report['flash_boot_count'], report['flash_injected_peer_session'], report['flash_done_seconds']), (3, None, None))
        ready = next(c for c in boot3 if c[3] == 'ready')
        start = next(c for c in boot3 if c[3] == 'start')
        cases = [
            ([c for c in boot3 if c[3] not in ('start', 'batch25', 'batch50', 'batch75', 'batch100', 'done')], 'flash_handshake_missing'),
            ([c for c in boot3 if c[3] != 'ready'], 'flash_handshake_missing'),
            (boot3 + [(start[0] + 0.01, b'PLFLS1 peer peer=000000040000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n', 3, 'x')], 'flash_handshake_malformed'),
            (boot3 + [(start[0] + 0.01, b'PLFLS1 done phase=8 fault=7 blocked=1\r\n', 3, 'x')], 'flash_handshake_malformed'),
            (boot3 + [(start[0] + 0.01, b'PLFLS1 error timeout ms=2500\r\n', 3, 'x')], 'flash_handshake_failed'),
            ([c if c[3] != 'start' else (ready[0] - 0.01, c[1], 3, 'start') for c in boot3], 'flash_handshake_order'),
            ([c if c[3] != 'ready' else (c[0] + 2.5, c[1], 3, 'ready') for c in boot3 if c[3] != 'start'] + [(ready[0] + 2.51, start[1], 3, 'start')],
             'flash_handshake_timing'),
            ([(t, d.replace(b'inactive=1', b'inactive=0'), b, k) for t, d, b, k in boot3], 'flash_handshake_failed'),
            ([(t, d.replace(b'count=3', b'count=4'), b, k) for t, d, b, k in boot3], 'flash_handshake_failed'),
        ]
        for received, outcome in cases:
            with self.subTest(outcome=outcome):
                result = self.startup_audit((received, end))
                self.assertEqual(result['outcome'], outcome)
                self.assertFalse(result['flash_handshake_verified'])

    def test_failures_have_distinct_outcomes(self):
        good, end = capture(self.identity)

        def at(label, boot):
            return next(c for c in good if c[2] == boot and c[3] == label)

        def edit(old, new, boot=None, label=None):
            return [(t, d.replace(old, new) if (boot in (None, b) and label in (None, k)) else d, b, k) for t, d, b, k in good], end

        def drop(boot, label):
            return [c for c in good if (c[2], c[3]) != (boot, label)], end

        def add(boot, label, data, delay=0.01):
            return good + [(at(label, boot)[0] + delay, data, boot, 'extra')], end
        no_reset = add(1, 'inject', b'PLPRK1 error no_reset ms=8000\r\n', 3.0)
        cases = [
            (([c for c in good if c[2] != 3], end), 'wrong_boot_count'),
            ((good + [(t + 200, d, 4, k) for t, d, b, k in good if b == 3], end + 200), 'wrong_boot_count'),
            (edit(b'count=2', b'count=3', 2, 'hello'), 'boot2_flash_handshake_failed'),
            (edit(b'PLLED1 done low=all\r\n', b'', 1), 'boot1_led_incomplete'),
            (drop(3, 'ready'), 'boot3_flash_handshake_missing'),
            (add(2, 'batch50', b'PLFLS1 error clock\r\n'), 'boot2_flash_handshake_failed'),
            (add(1, 'batch50', b'PLPRK1 error lease=37 stage=2 reply=1 phase=8 fault=7\r\n'), 'park_sequence'),
            (add(3, 'ready', b'PLPRK1 skip marker=00000003\r\n', 0.001), 'park_sequence'),
            (edit(b'leases=50 park_max', b'leases=x50 park_max', 2), 'park_sequence'),
            (add(2, 'batch25', b'PLPRK1 start mode=1 marker=00000001 watchdog=pass\r\n'), 'park_sequence'),
            (add(3, 'done', batch(25)), 'park_sequence'),
            (no_reset, 'park_reset_timing'),
            (edit(b'watchdog=pass', b'watchdog=fail', 2), 'park_start'),
            (edit(b'marker=00000001', b'marker=00000000', 2, 'start'), 'park_start'),
            (edit(b'mode=0 marker', b'mode=1 marker', 1), 'park_start'),
            (drop(3, 'batch75'), 'park_batches'),
            (add(1, 'batch100', batch(125)), 'park_batches'),
            (edit(b'frozen=75', b'frozen=74', 2), 'park_batches'),
            (edit(b'alive=75', b'alive=73', 1), 'park_batches'),
            (edit(b'park_max=310', b'park_max=0', 3, 'batch25'), 'park_batches'),
            (edit(b'release_max=290', b'release_max=1311', 1, 'batch100'), 'park_batches'),
            (edit(b'park_max=310', b'park_max=1311', 2, 'batch50'), 'park_batches'),
            (edit(b'begin=0', b'begin=1', 1), 'park_inject_ack'),
            (edit(b'lease=101', b'lease=100', 1), 'park_inject_ack'),
            (add(1, 'inject', b'PLPRK1 inject mode=0 lease=101 begin=0 peer=00000000 ms=0\r\n', 0.5), 'park_inject_ack'),
            (edit(b'peer=504b0001 ms=0', b'peer=504b0001 ms=5', 2), 'park_inject_ack'),
            (edit(b'mode=1 lease=101 begin=1 peer=504b0001', b'mode=0 lease=101 begin=1 peer=504b0001', 2), 'park_inject_ack'),
            (drop(2, 'release'), 'park_inject_release'),
            (edit(b'peer=504b0081', b'peer=504b0001', 2, 'release'), 'park_inject_release'),
            (edit(b'ms=705', b'ms=640', 2), 'park_inject_release'),
            (edit(b'ms=705', b'ms=770', 2), 'park_inject_release'),
            (capture(self.identity, reset1=1.5), 'park_reset_timing'),
            (capture(self.identity, reset1=6.5), 'park_reset_timing'),
            (capture(self.identity, reset2=1.9), 'park_reset_timing'),
            (capture(self.identity, reset2=7.0), 'park_reset_timing'),
            (edit(b'done leases=100', b'done leases=99'), 'park_done'),
            (edit(b'marker=00000002\r\n', b'marker=00000003\r\n', 3, 'done'), 'park_done'),
            (add(3, 'done', b'PLPRK1 done leases=100 marker=00000002\r\n'), 'park_done'),
            (add(3, 'done', b'PLFLS1 foreign\r\n'), 'park_done'),
            (capture(self.identity, tail=12.0), 'park_done'),
            (capture(self.identity, counts=(1, 1, 3)), 'park_sessions'),
            (capture(self.identity, peers=(2, 2, 3)), 'park_sessions'),
            (capture(self.identity, peers=(3, 2, 4)), 'park_sessions'),
        ]
        for received, outcome in cases:
            with self.subTest(outcome=outcome):
                report = self.audit(received)
                self.assertEqual(report['outcome'], outcome)
                self.assertNotIn('PLPRK1', json.dumps(report))

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        # A park image cannot be audited under the hello-only policy, or without a policy.
        self.policy('fw06-hello-v1')
        with self.assertRaisesRegex(ValueError, 'Park policy required'):
            self.audit(good)
        with self.assertRaisesRegex(ValueError, 'Flash park policy/artifact mismatch'):
            self.startup_audit(good)
        self.policy(None)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.startup_audit(good)
        self.policy('fw06-park-v2')
        with self.assertRaisesRegex(ValueError, 'Unknown flash handshake policy'):
            self.startup_audit(good)
        # A hello-only image cannot be audited under the park policy.
        self.policy('fw06-park-v1')
        self.image(self.base)
        for call in (self.audit, self.startup_audit):
            with self.assertRaisesRegex(ValueError, 'Park|park'):
                call(good)
        # The park policy still requires the HELLO record format.
        self.image(self.base.replace(S.FLASH_MARK, b'') + S.PARK_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.startup_audit(good)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.audit(good)


if __name__ == '__main__':
    unittest.main()
