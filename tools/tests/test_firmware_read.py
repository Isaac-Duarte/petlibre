from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_startup_read', TOOLS / 'audit/audit-firmware-startup.py')
S = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(S)
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
HELLO = b'PLFLS1 hello authority=000000010000a1b2 count=1\r\n'
READY = b'PLFLS1 ready peer=000000010000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n'
START = b'PLRDS1 start marker=00000000 watchdog=pass installed=1\r\n'
DONE = b'PLRDS1 done reads=100 leases=100 phase=1 fault=0\r\n'


def batch(reads, ident=b'c84017', mismatches=0, park=6, release=2, frozen=None, alive=None):
    return b'PLRDS1 batch reads=%d id=%s mismatches=%d park_max=%d release_max=%d frozen=%d alive=%d\r\n' % (
        reads, ident, mismatches, park, release, reads if frozen is None else frozen, reads if alive is None else alive)


def capture(identity, read_seconds=8.0, tail=30.0, ready_delay=0.3):
    """(host seconds, bytes, label) receive chunks of one FW06 mediated-read boot, and the capture end."""
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000\r\n'
    boot = (POWER + CLOCK + ROUTING + OUTPUTS + b'Petlibre local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)
    out = [(0.0, boot, 'startup'), (3.0, LED_BEGIN, 'led'), (17.0, LED_REST, 'led'), (17.2, HELLO, 'hello'),
           (17.2 + ready_delay, READY, 'ready')]
    t = out[-1][0] + 0.05
    out.append((t, START, 'start'))
    for n, reads in enumerate(S.READ_BATCHES, 1):
        out.append((t + read_seconds * n / 4 - 0.01 * (n == 4), batch(reads), 'batch%d' % reads))
    out.append((t + read_seconds, DONE, 'done'))
    return out, t + read_seconds + tail


class FirmwareRead(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy='fw06-read-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT + b' ' + S.FLASH_MARK)
        self.image(self.base + b' ' + S.READ_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def policy(self, policy):
        self.manifest['startup_flash_handshake_policy'] = policy
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def audit(self, received):
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
        return S.verify(self.p, self.p / 'manifest.json', self.p / 'image.bin', 1)

    def test_single_boot_read_passes_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertTrue(report['flash_handshake_verified'])
        self.assertTrue(report['flash_read_verified'])
        self.assertEqual((report['flash_read_id'], report['flash_read_reads'], report['flash_read_leases']), ('c84017', 100, 100))
        self.assertEqual((report['flash_read_park_max'], report['flash_read_release_max']), (6, 2))
        self.assertEqual((report['flash_read_frozen'], report['flash_read_alive']), (100, 100))
        self.assertAlmostEqual(report['flash_read_seconds'], 8.0, places=3)
        self.assertAlmostEqual(report['flash_read_tail_seconds'], 30.0, places=3)
        self.assertEqual((report['flash_authority_session'], report['flash_boot_count'], report['flash_peer_session']),
                         ('000000010000a1b2', 1, '000000010000c3d4'))
        self.assertIsNone(report['flash_injected_peer_session'])
        self.assertNotIn('PLRDS1', json.dumps(report))
        # One foreign line, alive one below reads, bounds at the limit and unrelated output after done still pass.
        chunks, end = capture(self.identity, read_seconds=30.0)
        chunks = [(t, batch(50, alive=49, park=1310, release=1310) if k == 'batch50' else d, k) for t, d, k in chunks]
        chunks += [(17.3, b'PLFLS1 foreign\r\n', 'foreign'), (end - 1, b'unrelated log\r\n', 'tail')]
        self.assertEqual(self.audit((chunks, end))['outcome'], 'pass')

    def test_failures_have_distinct_outcomes(self):
        good, end = capture(self.identity)

        def at(label):
            return next(c for c in good if c[2] == label)

        def edit(label, old, new):
            return [(t, d.replace(old, new) if k == label else d, k) for t, d, k in good], end

        def drop(*labels):
            return [c for c in good if c[2] not in labels], end

        def add(label, data, delay=0.001):
            return good + [(at(label)[0] + delay, data, 'extra')], end

        def swap(label, data):
            return [(t, data if k == label else d, k) for t, d, k in good], end
        cases = [
            (drop('start', 'batch25', 'batch50', 'batch75', 'batch100', 'done'), 'flash_read_missing'),
            (drop('done'), 'flash_read_missing'),
            (drop('start'), 'flash_read_missing'),
            (add('batch50', b'PLRDS1 untrusted\r\n'), 'flash_read_malformed'),
            (edit('batch25', b'id=c84017', b'id=C84017'), 'flash_read_malformed'),
            (edit('batch25', b'id=c84017', b'id=c8401'), 'flash_read_malformed'),
            (add('start', START), 'flash_read_malformed'),
            (add('batch100', DONE), 'flash_read_malformed'),
            (add('batch25', batch(25)), 'flash_read_malformed'),
            (add('ready', b'PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n'), 'flash_read_malformed'),
            (add('done', b'PLPRK1 done leases=100 marker=00000002\r\n'), 'flash_read_malformed'),
            (add('ready', b'PLRDS1 skip marker=00000001\r\n'), 'flash_read_failed'),
            (add('batch50', b'PLFLS1 error timeout ms=2500\r\n'), 'flash_read_failed'),
            (edit('start', b'marker=00000000', b'marker=00000001'), 'flash_read_failed'),
            (edit('start', b'watchdog=pass', b'watchdog=fail'), 'flash_read_failed'),
            (edit('start', b'installed=1', b'installed=0'), 'flash_read_failed'),
            (drop('batch75'), 'flash_read_failed'),
            (add('batch100', batch(125)), 'flash_read_failed'),
            (swap('batch50', batch(50, ident=b'ef4017')), 'flash_read_failed'),
            (swap('batch50', batch(50, mismatches=1)), 'flash_read_failed'),
            (swap('batch75', batch(75, frozen=74)), 'flash_read_failed'),
            (swap('batch75', batch(75, alive=73)), 'flash_read_failed'),
            (swap('batch25', batch(25, park=0)), 'flash_read_failed'),
            (swap('batch25', batch(25, park=1311)), 'flash_read_failed'),
            (swap('batch100', batch(100, release=0)), 'flash_read_failed'),
            (swap('batch100', batch(100, release=1311)), 'flash_read_failed'),
            (edit('done', b'reads=100', b'reads=99'), 'flash_read_failed'),
            (edit('done', b'leases=100', b'leases=99'), 'flash_read_failed'),
            (edit('done', b'phase=1', b'phase=8'), 'flash_read_failed'),
            (edit('done', b'fault=0', b'fault=7'), 'flash_read_failed'),
            (([c if c[2] != 'start' else (at('ready')[0] - 0.01, c[1], 'start') for c in good], end), 'flash_read_order'),
            (([c if c[2] != 'batch25' else (at('batch75')[0] + 0.01, c[1], 'batch25') for c in good], end), 'flash_read_order'),
            (([c if c[2] != 'batch100' else (at('done')[0] + 0.01, c[1], 'batch100') for c in good], end), 'flash_read_order'),
            (([c if c[2] != 'batch25' else (at('start')[0] - 0.001, c[1], 'batch25') for c in good], end), 'flash_read_order'),
            (add('done', b'PLFLS1 foreign\r\n', 1.0), 'flash_read_order'),
            (capture(self.identity, read_seconds=30.1), 'flash_read_timing'),
            (capture(self.identity, tail=29.9), 'flash_read_timing'),
            (drop('ready'), 'flash_handshake_missing'),
            (drop('hello'), 'flash_handshake_missing'),
            (add('batch50', b'PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n'), 'flash_handshake_malformed'),
            (add('done', b'PLFLS1 done phase=8 fault=7 blocked=1\r\n'), 'flash_handshake_malformed'),
            (add('hello', b'PLFLS1 error clock\r\n'), 'flash_handshake_failed'),
            (edit('hello', b'count=1', b'count=2'), 'flash_handshake_failed'),
            (edit('ready', b'inactive=1', b'inactive=0'), 'flash_handshake_failed'),
            (add('led', b'PLFLS1 foreign\r\n', 0.1), 'flash_handshake_order'),
            (capture(self.identity, ready_delay=2.6), 'flash_handshake_timing'),
        ]
        for received, outcome in cases:
            with self.subTest(outcome=outcome):
                report = self.audit(received)
                self.assertEqual(report['outcome'], outcome)
                self.assertFalse(report['flash_read_verified'])
                self.assertNotIn('PLRDS1', json.dumps(report))

    def test_reset_is_a_repeated_startup(self):
        chunks, end = capture(self.identity)
        again = [(t + end, d, k) for t, d, k in chunks]
        self.assertEqual(self.audit((chunks + again, end * 2))['outcome'], 'repeated_startup')

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        # A read image cannot be audited under another flash policy, or without one.
        for policy, message in (('fw06-hello-v1', 'Flash read policy/artifact mismatch'), ('fw06-park-v1', 'Flash park policy/artifact mismatch'),
                                (None, 'Flash handshake policy/artifact mismatch'), ('fw06-read-v2', 'Unknown flash handshake policy')):
            with self.subTest(policy=policy):
                self.policy(policy)
                with self.assertRaisesRegex(ValueError, message):
                    self.audit(good)
        # A hello-only or park image cannot be audited under the read policy.
        self.policy('fw06-read-v1')
        for image in (self.base, self.base + b' ' + S.PARK_MARK):
            with self.subTest(image=image[-20:]):
                self.image(image)
                with self.assertRaisesRegex(ValueError, 'Flash (read|park) policy/artifact mismatch'):
                    self.audit(good)
        # The read policy still requires the HELLO record format.
        self.image(self.base.replace(S.FLASH_MARK, b'') + S.READ_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.audit(good)
        # The classifier needs the capture end for the clean tail.
        self.image(self.base + b' ' + S.READ_MARK)
        rx = b''.join(c[1] for c in good[0])
        with self.assertRaisesRegex(ValueError, 'capture end time'):
            S.startup(rx, self.manifest, (self.p / 'image.bin').read_bytes(), self.identity, lambda offset: T0)

    def test_hello_and_park_policies_unaffected(self):
        # PLRDS1 text is ignored (and produces no read fields) under the earlier policies.
        def run(lines, version, times):
            rx = LED_REST + b''.join(lines)
            marks = [rx.index(line) for line in lines]
            return S.classify_flash(rx, lambda offset: T0 + timedelta(seconds=next(
                t for m, t in reversed(list(zip(marks, times))) if m <= offset)), version)
        peer = b'PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n'
        done = b'PLFLS1 done phase=8 fault=7 blocked=1\r\n'
        hello = run([HELLO, READY, START, peer, done], 1, (0, 1, 1, 11, 31))
        self.assertEqual(hello['outcome'], 'pass', hello)
        park = run([HELLO, READY, b'PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n', START], 2, (0, 1, 1, 1))
        self.assertEqual(park['outcome'], 'pass', park)
        for report in (hello, park):
            self.assertFalse(any(key.startswith('flash_read_') for key in report))


if __name__ == '__main__':
    unittest.main()
