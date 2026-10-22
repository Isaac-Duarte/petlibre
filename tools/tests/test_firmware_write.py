from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_startup_write', TOOLS / 'audit/audit-firmware-startup.py')
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
START = b'PLWRS1 start marker=00000000 watchdog=pass installed=1\r\n'


def preimage(ident=b'c84017', blank=1024):
    return b'PLWRS1 preimage id=%s blank=%d\r\n' % (ident, blank)


def cycle(n, erase=1500, blank=1024, program=40, errors=0):
    return b'PLWRS1 cycle n=%d erase_ticks=%d blank=%d program_max=%d errors=%d\r\n' % (n, erase, blank, program, errors)


def restore(erase=1600, blank=1024):
    return b'PLWRS1 restore erase_ticks=%d blank=%d\r\n' % (erase, blank)


def done(cycles=3, leases=53, phase=1, fault=0, park=6, release=2, frozen=53):
    return b'PLWRS1 done cycles=%d leases=%d phase=%d fault=%d park_max=%d release_max=%d frozen=%d\r\n' % (
        cycles, leases, phase, fault, park, release, frozen)


def capture(identity, write_seconds=10.0, tail=30.0, ready_delay=0.3):
    """(host seconds, bytes, label) receive chunks of one FW06 scratch-sector write boot, and the capture end."""
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000\r\n'
    boot = (POWER + CLOCK + ROUTING + OUTPUTS + b'Petlibre local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)
    out = [(0.0, boot, 'startup'), (3.0, LED_BEGIN, 'led'), (17.0, LED_REST, 'led'), (17.2, HELLO, 'hello'),
           (17.2 + ready_delay, READY, 'ready')]
    t = out[-1][0] + 0.05
    out += [(t, START, 'start'), (t + 0.1, preimage(), 'preimage')]
    for n in S.WRITE_CYCLES:
        out.append((t + write_seconds * n / 5, cycle(n), 'cycle%d' % n))
    out += [(t + write_seconds * 4 / 5, restore(), 'restore'), (t + write_seconds, done(), 'done')]
    return out, t + write_seconds + tail


class FirmwareWrite(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy='fw06-write-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT + b' ' + S.FLASH_MARK)
        self.image(self.base + b' ' + S.WRITE_MARK)

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

    def test_single_boot_write_passes_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertTrue(report['flash_handshake_verified'])
        self.assertTrue(report['flash_write_verified'])
        self.assertEqual((report['flash_write_id'], report['flash_write_preimage_blank']), ('c84017', 1024))
        self.assertEqual(report['flash_write_cycles'], [{'n': n, 'erase_ticks': 1500, 'blank': 1024, 'program_max': 40, 'errors': 0}
                                                        for n in (1, 2, 3)])
        self.assertEqual((report['flash_write_restore_erase_ticks'], report['flash_write_restore_blank']), (1600, 1024))
        self.assertEqual((report['flash_write_leases'], report['flash_write_frozen']), (53, 53))
        self.assertEqual((report['flash_write_park_max'], report['flash_write_release_max']), (6, 2))
        self.assertEqual(report['flash_write_erase_ms_max'], 48.8)
        self.assertAlmostEqual(report['flash_write_seconds'], 10.0, places=3)
        self.assertAlmostEqual(report['flash_write_tail_seconds'], 30.0, places=3)
        self.assertEqual((report['flash_authority_session'], report['flash_boot_count'], report['flash_peer_session']),
                         ('000000010000a1b2', 1, '000000010000c3d4'))
        self.assertFalse(any(key.startswith('flash_read_') for key in report))
        self.assertNotIn('PLWRS1', json.dumps(report))
        # One foreign line, bounds at the limits and unrelated output after done still pass.
        chunks, end = capture(self.identity, write_seconds=60.0)
        swaps = {'cycle2': cycle(2, erase=16384, program=1638), 'restore': restore(erase=16384), 'done': done(park=1310, release=1310)}
        chunks = [(t, swaps.get(k, d), k) for t, d, k in chunks]
        chunks += [(17.3, b'PLFLS1 foreign\r\n', 'foreign'), (end - 1, b'unrelated log\r\n', 'tail')]
        report = self.audit((chunks, end))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['flash_write_erase_ms_max'], 500.0)

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

        def move(label, before):
            return [c if c[2] != label else (at(before)[0] - 0.001, c[1], label) for c in good], end
        cases = [
            (drop('start', 'preimage', 'cycle1', 'cycle2', 'cycle3', 'restore', 'done'), 'flash_write_missing'),
            (drop('start'), 'flash_write_missing'),
            (drop('preimage'), 'flash_write_missing'),
            (drop('restore'), 'flash_write_missing'),
            (drop('done'), 'flash_write_missing'),
            (add('cycle1', b'PLWRS1 untrusted\r\n'), 'flash_write_malformed'),
            (edit('preimage', b'id=c84017', b'id=C84017'), 'flash_write_malformed'),
            (edit('cycle2', b'errors=0', b'errors=-1'), 'flash_write_malformed'),
            (add('start', START), 'flash_write_malformed'),
            (add('preimage', preimage()), 'flash_write_malformed'),
            (add('restore', restore()), 'flash_write_malformed'),
            (add('done', done()), 'flash_write_malformed'),
            (add('cycle1', cycle(1)), 'flash_write_malformed'),
            (add('ready', b'PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n'), 'flash_write_malformed'),
            (add('cycle1', b'PLRDS1 batch reads=25 id=c84017 mismatches=0 park_max=6 release_max=2 frozen=25 alive=25\r\n'),
             'flash_write_malformed'),
            (add('ready', b'PLWRS1 skip marker=00000001\r\n'), 'flash_write_failed'),
            (add('cycle2', b'PLFLS1 error timeout ms=2500\r\n'), 'flash_write_failed'),
            (add('done', b'PLFLS1 error clock\r\n', 1.0), 'flash_write_failed'),
            (edit('start', b'marker=00000000', b'marker=00000001'), 'flash_write_failed'),
            (edit('start', b'watchdog=pass', b'watchdog=fail'), 'flash_write_failed'),
            (edit('start', b'installed=1', b'installed=0'), 'flash_write_failed'),
            (swap('preimage', preimage(blank=1023)), 'flash_write_failed'),
            (swap('preimage', preimage(ident=b'ef4017')), 'flash_write_failed'),
            (drop('cycle3'), 'flash_write_failed'),
            (add('cycle3', cycle(4)), 'flash_write_failed'),
            (swap('cycle2', cycle(2, errors=1)), 'flash_write_failed'),
            (swap('cycle2', cycle(2, blank=1023)), 'flash_write_failed'),
            (swap('cycle1', cycle(1, erase=16385)), 'flash_write_failed'),
            (swap('cycle1', cycle(1, erase=0)), 'flash_write_failed'),
            (swap('cycle3', cycle(3, program=1639)), 'flash_write_failed'),
            (swap('cycle3', cycle(3, program=0)), 'flash_write_failed'),
            (swap('restore', restore(blank=1000)), 'flash_write_failed'),
            (swap('restore', restore(erase=16385)), 'flash_write_failed'),
            (swap('restore', restore(erase=0)), 'flash_write_failed'),
            (swap('done', done(cycles=2)), 'flash_write_failed'),
            (swap('done', done(leases=52)), 'flash_write_failed'),
            (swap('done', done(phase=8)), 'flash_write_failed'),
            (swap('done', done(fault=7)), 'flash_write_failed'),
            (swap('done', done(frozen=52)), 'flash_write_failed'),
            (swap('done', done(park=0)), 'flash_write_failed'),
            (swap('done', done(park=1311)), 'flash_write_failed'),
            (swap('done', done(release=0)), 'flash_write_failed'),
            (swap('done', done(release=1311)), 'flash_write_failed'),
            (move('start', 'ready'), 'flash_write_order'),
            (move('preimage', 'start'), 'flash_write_order'),
            (move('cycle1', 'preimage'), 'flash_write_order'),
            (move('cycle1', 'cycle3'), 'flash_write_order'),
            (([c if c[2] != 'cycle3' else (at('restore')[0] + 0.001, c[1], 'cycle3') for c in good], end), 'flash_write_order'),
            (([c if c[2] != 'restore' else (at('done')[0] + 0.001, c[1], 'restore') for c in good], end), 'flash_write_order'),
            (add('done', b'PLFLS1 foreign\r\n', 1.0), 'flash_write_order'),
            (capture(self.identity, write_seconds=60.1), 'flash_write_timing'),
            (capture(self.identity, tail=29.9), 'flash_write_timing'),
            (drop('ready'), 'flash_handshake_missing'),
            (drop('hello'), 'flash_handshake_missing'),
            (add('cycle1', b'PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n'), 'flash_handshake_malformed'),
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
                self.assertFalse(report['flash_write_verified'])
                self.assertNotIn('PLWRS1', json.dumps(report))

    def test_reset_is_a_repeated_startup(self):
        chunks, end = capture(self.identity)
        again = [(t + end, d, k) for t, d, k in chunks]
        self.assertEqual(self.audit((chunks + again, end * 2))['outcome'], 'repeated_startup')

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        # A write image cannot be audited under another flash policy, or without one.
        for policy, message in (('fw06-hello-v1', 'Flash write policy/artifact mismatch'), ('fw06-park-v1', 'Flash park policy/artifact mismatch'),
                                ('fw06-read-v1', 'Flash read policy/artifact mismatch'), (None, 'Flash handshake policy/artifact mismatch'),
                                ('fw06-write-v2', 'Unknown flash handshake policy')):
            with self.subTest(policy=policy):
                self.policy(policy)
                with self.assertRaisesRegex(ValueError, message):
                    self.audit(good)
        # A hello-only, park or read image cannot be audited under the write policy.
        self.policy('fw06-write-v1')
        for image in (self.base, self.base + b' ' + S.PARK_MARK, self.base + b' ' + S.READ_MARK):
            with self.subTest(image=image[-20:]):
                self.image(image)
                with self.assertRaisesRegex(ValueError, 'Flash (write|park|read) policy/artifact mismatch'):
                    self.audit(good)
        # The write policy still requires the HELLO record format.
        self.image(self.base.replace(S.FLASH_MARK, b'') + S.WRITE_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.audit(good)
        # The classifier needs the capture end for the clean tail.
        self.image(self.base + b' ' + S.WRITE_MARK)
        rx = b''.join(c[1] for c in good[0])
        with self.assertRaisesRegex(ValueError, 'capture end time'):
            S.startup(rx, self.manifest, (self.p / 'image.bin').read_bytes(), self.identity, lambda offset: T0)

    def test_earlier_policies_unaffected(self):
        # PLWRS1 text is ignored (and produces no write fields) under the earlier policies.
        def run(lines, version, times, end=None):
            rx = LED_REST + b''.join(lines)
            marks = [rx.index(line) for line in lines]
            return S.classify_flash(rx, lambda offset: T0 + timedelta(seconds=next(
                t for m, t in reversed(list(zip(marks, times))) if m <= offset)), version, end)
        peer = b'PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n'
        fin = b'PLFLS1 done phase=8 fault=7 blocked=1\r\n'
        hello = run([HELLO, READY, START, peer, fin], 1, (0, 1, 1, 11, 31))
        self.assertEqual(hello['outcome'], 'pass', hello)
        park = run([HELLO, READY, b'PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n', START], 2, (0, 1, 1, 1))
        self.assertEqual(park['outcome'], 'pass', park)
        read_lines = [b'PLRDS1 start marker=00000000 watchdog=pass installed=1\r\n'] + [
            b'PLRDS1 batch reads=%d id=c84017 mismatches=0 park_max=6 release_max=2 frozen=%d alive=%d\r\n' % (n, n, n)
            for n in S.READ_BATCHES] + [b'PLRDS1 done reads=100 leases=100 phase=1 fault=0\r\n']
        read = run([HELLO, READY, START] + read_lines, 3, (0, 1, 1, 1, 2, 3, 4, 5, 6), T0 + timedelta(seconds=40))
        self.assertEqual(read['outcome'], 'pass', read)
        for report in (hello, park, read):
            self.assertFalse(any(key.startswith('flash_write_') for key in report))


if __name__ == '__main__':
    unittest.main()
