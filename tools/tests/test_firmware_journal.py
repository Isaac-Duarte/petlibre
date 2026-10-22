from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('firmware_journal', TOOLS / 'audit/audit-firmware-journal.py')
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
VIEW = b'PLJNL1 view sequence=6 high_water=3 pending=1 result=3 outcome=0 slot=1\r\n'
REFUSE = b'PLJNL1 refuse id=4 reserved=0\r\n'
WRITE_TICKS = 9830  # 300 ms


def startup(identity, reset):
    start = b'PLFW1 start build=' + identity.encode() + b' bank=1 reset=' + reset + b' mmu=00000003,0c000000,03e7a000\r\n'
    return (POWER + CLOCK + ROUTING + OUTPUTS + b'Wellfed local firmware: Rust bringup\r\n' + start
            + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + S.PASS)


def op(kind, ident, ticks, ok=1, error=0):
    return b'PLJNL1 op kind=%d id=%d ok=%d error=%d ticks=%d\r\n' % (kind, ident, ok, error, ticks)


def end(mode, reset, erases, leases=13):
    return b'PLJNL1 end mode=%d reset=%d leases=%d frozen=%d erases=%d phase=1 fault=0\r\n' % (mode, reset, leases, leases, erases)


def capture(identity, reset1=3.5, reset2=3.6, tail=40.0, counts=(1, 2, 3), peers=(1, 2, 3)):
    """(host seconds, bytes, boot, label) receive chunks of a realistic three-boot FW15 journal capture."""
    out, t0 = [], 0.0
    for boot, (count, peer) in enumerate(zip(counts, peers), 1):
        mode = boot - 1
        reset = b'00000000' if boot == 1 else b'00100000'
        out += [(t0, startup(identity, reset), boot, 'startup'), (t0 + 3.0, LED_BEGIN, boot, 'led'), (t0 + 17.0, LED_REST, boot, 'led')]
        t = t0 + 17.2
        out.append((t, b'PLFLS1 hello authority=%08x0000a1b2 count=%d\r\n' % (count, count), boot, 'hello'))
        t += 0.3
        out.append((t, b'PLFLS1 ready peer=%08x0000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n' % peer, boot, 'ready'))
        t += 0.05
        start = b'PLJNL1 start mode=%d marker=%08x watchdog=pass installed=1\r\n' % (mode, mode)
        if boot == 1:
            # The start and preimage records share one receive chunk.
            out.append((t, start + b'PLJNL1 preimage id=c84017 blank_sectors=16\r\n', boot, 'start'))
            ops = [(0, 0, 0), (1, 1, WRITE_TICKS), (2, 1, WRITE_TICKS - 30), (1, 2, WRITE_TICKS + 12), (2, 2, WRITE_TICKS), (1, 3, WRITE_TICKS + 5)]
            for n, (kind, ident, ticks) in enumerate(ops):
                t += 0.32
                out.append((t, op(kind, ident, ticks), boot, f'op{n}'))
            t += 0.05
            out.append((t, VIEW, boot, 'view'))
            t += 0.05
            out.append((t, end(0, 1, 5), boot, 'end'))
            t0 = t + reset1
        else:
            out.append((t, start, boot, 'start'))
            t += 0.05
            out.append((t, op(3, 0, 0), boot, 'open'))
            t += 0.05
            out.append((t, VIEW + REFUSE, boot, 'view'))
            t += 0.4
            out.append((t, op(4, 0, 0) if boot == 2 else op(5, 0, 14000), boot, 'last'))
            t += 0.05
            out.append((t, end(mode, 1, 1) if boot == 2 else end(2, 0, 2), boot, 'end'))
            t0 = t + reset2
            finished = t + tail
    return out, finished


class FirmwareJournal(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy='fw15-journal-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT + b' ' + S.FLASH_MARK)
        self.image(self.base + b' ' + S.JOURNAL_MARK)

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

    def test_three_boot_capture_passes_with_fixed_fields(self):
        report = self.audit(capture(self.identity))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['startups'], 3)
        self.assertEqual([report[f'boot{n}']['outcome'] for n in (1, 2, 3)], ['pass'] * 3)
        self.assertTrue(all(report[f'boot{n}']['flash_handshake_verified'] for n in (1, 2, 3)))
        self.assertEqual(report['boot_reset_flags'], ['00000000', '00100000', '00100000'])
        self.assertEqual(report['boot_hello_counts'], [1, 2, 3])
        self.assertEqual(report['boot_peer_session_high_words'], [1, 2, 3])
        self.assertEqual([[(o['kind'], o['id'], o['ok'], o['error']) for o in boot] for boot in report['boot_ops']],
                         [[(0, 0, 1, 0), (1, 1, 1, 0), (2, 1, 1, 0), (1, 2, 1, 0), (2, 2, 1, 0), (1, 3, 1, 0)],
                          [(3, 0, 1, 0), (4, 0, 1, 0)], [(3, 0, 1, 0), (5, 0, 1, 0)]])
        self.assertEqual(report['boot_views'], [dict(sequence=6, high_water=3, pending=1, result=3, outcome=0, slot=1)] * 3)
        self.assertEqual([(e['mode'], e['reset'], e['erases'], e['leases'], e['frozen']) for e in report['boot_ends']],
                         [(0, 1, 5, 13, 13), (1, 1, 1, 13, 13), (2, 0, 2, 13, 13)])
        self.assertEqual(len(report['journal_reset_seconds']), 2)
        self.assertAlmostEqual(report['journal_reset_seconds'][0], 3.5, places=3)
        self.assertAlmostEqual(report['journal_reset_seconds'][1], 3.6, places=3)
        self.assertEqual(report['journal_write_ms_max'], round((WRITE_TICKS + 12) * 1000 / 32768, 1))
        self.assertAlmostEqual(report['journal_tail_seconds'], 40.0, places=3)
        self.assertIn('USB latency', report['limit'])
        self.assertIn('power-cut', report['limit'])
        # The single-boot auditor keeps its semantics: three startups are a repeated startup.
        self.assertEqual(self.startup_audit(capture(self.identity))['outcome'], 'repeated_startup')
        # A one-foreign-line handshake, any create/open/torn ticks and the write bound itself still pass.
        chunks, finished = capture(self.identity)
        chunks = [(t, d.replace(b'ticks=%d' % WRITE_TICKS, b'ticks=26214'), b, k) for t, d, b, k in chunks]
        chunks = [(t, d.replace(b'kind=3 id=0 ok=1 error=0 ticks=0', b'kind=3 id=0 ok=1 error=0 ticks=9'), b, k) for t, d, b, k in chunks]
        chunks.append((chunks[3][0] + 0.1, b'PLFLS1 foreign\r\n', 1, 'foreign'))
        report = self.audit((chunks, finished))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['journal_write_ms_max'], 800.0)

    def test_reset_timed_from_next_boot_chunk_when_split_shares_a_chunk(self):
        # Records start with '\r', so the old boot's last chunk can end with the
        # next record's leading byte: the split then falls inside that chunk.
        chunks, finished = capture(self.identity)
        chunks = [(t, d + b'\r' if k == 'end' and b < 3 else d, b, k) for t, d, b, k in chunks]
        report = self.audit((chunks, finished))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertAlmostEqual(report['journal_reset_seconds'][0], 3.5, places=3)
        self.assertAlmostEqual(report['journal_reset_seconds'][1], 3.6, places=3)

    def test_single_boot_journal_handshake_under_startup_auditor(self):
        chunks, finished = capture(self.identity)
        boot3 = [c for c in chunks if c[2] == 3]
        report = self.startup_audit((boot3, finished))
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertTrue(report['flash_handshake_verified'])
        self.assertEqual(report['flash_boot_count'], 3)
        ready = next(c for c in boot3 if c[3] == 'ready')
        start = next(c for c in boot3 if c[3] == 'start')
        cases = [
            ([c for c in boot3 if c[3] not in ('start', 'open', 'view', 'last', 'end')], 'flash_handshake_missing'),
            ([c for c in boot3 if c[3] != 'hello'], 'flash_handshake_missing'),
            ([c if c[3] != 'start' else (ready[0] - 0.01, c[1], 3, 'start') for c in boot3], 'flash_handshake_order'),
            ([c if c[3] != 'ready' else (c[0] + 2.5, c[1], 3, 'ready') for c in boot3 if c[3] not in ('start', 'open', 'view', 'last', 'end')]
             + [(ready[0] + 2.51, start[1], 3, 'start')], 'flash_handshake_timing'),
            ([(t, d.replace(b'reply=0', b'reply=1'), b, k) for t, d, b, k in boot3], 'flash_handshake_failed'),
            (boot3 + [(start[0] + 0.01, b'PLFLS1 error clock\r\n', 3, 'x')], 'flash_handshake_failed'),
            (boot3 + [(start[0] + 0.01, b'PLFLS1 done phase=8 fault=7 blocked=1\r\n', 3, 'x')], 'flash_handshake_malformed'),
        ] + [(boot3 + [(start[0] + 0.01, line, 3, 'x')], 'flash_handshake_malformed') for line in (
            b'PLPRK1 start mode=2 marker=00000002 watchdog=pass\r\n',
            b'PLRDS1 start marker=00000000 watchdog=pass installed=1\r\n',
            b'PLWRS1 start marker=00000000 watchdog=pass installed=1\r\n')]
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
        cases = [
            (([c for c in good if c[2] != 3], finished), 'wrong_boot_count'),
            ((good + [(t + 200, d, 4, k) for t, d, b, k in good if b == 3], finished + 200), 'wrong_boot_count'),
            (edit(b'count=2', b'count=3', 2, 'hello'), 'boot2_flash_handshake_failed'),
            (edit(b'PLLED1 done low=all\r\n', b'', 1), 'boot1_led_incomplete'),
            (drop(3, 'ready'), 'boot3_flash_handshake_missing'),
            (edit(b'inactive=1', b'inactive=0', 2), 'boot2_flash_handshake_failed'),
            (add(2, 'open', b'PLFLS1 error clock\r\n'), 'journal_sequence'),
            (add(1, 'op2', b'PLJNL1 skip marker=00000001\r\n'), 'journal_sequence'),
            (edit(b'blank_sectors=16', b'blank_sectors=x16', 1), 'journal_sequence'),
            (edit(b'ok=1 error=0 ticks=0', b'ok=1 error=0 ticks=-1', 2, 'open'), 'journal_sequence'),
            (add(2, 'open', b'PLJNL1 start mode=1 marker=00000001 watchdog=pass installed=1\r\n'), 'journal_sequence'),
            (add(1, 'view', VIEW), 'journal_sequence'),
            (add(3, 'end', op(5, 0, 100)), 'journal_sequence'),
            (edit(b'watchdog=pass', b'watchdog=fail', 2), 'journal_start'),
            (edit(b'installed=1', b'installed=0', 3), 'journal_start'),
            (edit(b'marker=00000001', b'marker=00000000', 2, 'start'), 'journal_start'),
            (edit(b'mode=0 marker', b'mode=1 marker', 1), 'journal_start'),
            (edit(b'PLJNL1 preimage id=c84017 blank_sectors=16\r\n', b'', 1), 'journal_preimage'),
            (edit(b'blank_sectors=16', b'blank_sectors=15', 1), 'journal_preimage'),
            (edit(b'id=c84017', b'id=ef4017', 1), 'journal_preimage'),
            (add(2, 'start', b'PLJNL1 preimage id=c84017 blank_sectors=16\r\n'), 'journal_preimage'),
            (drop(1, 'op3'), 'journal_ops'),
            (drop(2, 'open'), 'journal_ops'),
            (drop(3, 'last'), 'journal_ops'),
            (edit(op(4, 0, 0), op(5, 0, 100), 2), 'journal_ops'),
            (edit(op(4, 0, 0), op(4, 0, 0, ok=0), 2), 'journal_ops'),
            (edit(op(3, 0, 0), op(3, 0, 0, error=3), 3), 'journal_ops'),
            (edit(op(1, 3, WRITE_TICKS + 5), op(1, 4, WRITE_TICKS + 5), 1), 'journal_ops'),
            (edit(op(1, 2, WRITE_TICKS + 12), op(1, 2, 26215), 1), 'journal_ops'),
            (edit(op(2, 2, WRITE_TICKS), op(2, 2, 0), 1), 'journal_ops'),
            (edit(op(5, 0, 14000), op(5, 0, 0), 3), 'journal_ops'),
            (edit(op(3, 0, 0), op(7, 0, 0), 3), 'journal_ops'),
            (add(1, 'view', op(1, 4, WRITE_TICKS), 0.02), 'journal_ops'),
            (edit(op(3, 0, 0), op(3, 0, 0) + op(4, 0, 0), 2), 'journal_ops'),
            (drop(1, 'view'), 'journal_view'),
            (edit(VIEW + REFUSE, REFUSE, 3), 'journal_view'),
            (edit(b'pending=1', b'pending=0', 2), 'journal_view'),
            (edit(b'sequence=6', b'sequence=7', 3), 'journal_view'),
            (edit(b'slot=1', b'slot=0', 1), 'journal_view'),
            (edit(VIEW + REFUSE, VIEW, 2), 'journal_refuse'),
            (edit(b'reserved=0', b'reserved=1', 3), 'journal_refuse'),
            (edit(b'refuse id=4', b'refuse id=3', 2), 'journal_refuse'),
            (add(1, 'view', REFUSE, 0.02), 'journal_refuse'),
            (drop(3, 'end'), 'journal_end'),
            (edit(end(0, 1, 5), end(0, 1, 4), 1), 'journal_end'),
            (edit(end(1, 1, 1), end(1, 0, 1), 2), 'journal_end'),
            (edit(end(2, 0, 2), end(2, 0, 2, leases=0), 3), 'journal_end'),
            (edit(b'frozen=13', b'frozen=12', 2), 'journal_end'),
            (edit(b'phase=1 fault=0\r\n', b'phase=8 fault=0\r\n', 3, 'end'), 'journal_end'),
            (edit(b'phase=1 fault=0\r\n', b'phase=1 fault=7\r\n', 1, 'end'), 'journal_end'),
            (edit(end(2, 0, 2), end(1, 0, 2), 3), 'journal_end'),
            (capture(self.identity, reset1=1.9), 'journal_reset_timing'),
            (capture(self.identity, reset1=6.6), 'journal_reset_timing'),
            (capture(self.identity, reset2=1.5), 'journal_reset_timing'),
            (capture(self.identity, reset2=7.0), 'journal_reset_timing'),
            (capture(self.identity, tail=29.0), 'journal_tail'),
            (add(3, 'end', b'PLFLS1 foreign\r\n', 5.0), 'journal_tail'),
            (capture(self.identity, counts=(1, 1, 3)), 'journal_sessions'),
            (capture(self.identity, peers=(2, 2, 3)), 'journal_sessions'),
            (capture(self.identity, peers=(3, 2, 4)), 'journal_sessions'),
        ]
        for received, outcome in cases:
            with self.subTest(outcome=outcome):
                report = self.audit(received)
                self.assertEqual(report['outcome'], outcome)
                self.assertNotIn('PLJNL1', json.dumps(report))
        # The reset bounds themselves pass.
        for reset in (2.0, 6.5):
            with self.subTest(reset=reset):
                self.assertEqual(self.audit(capture(self.identity, reset1=reset, reset2=reset))['outcome'], 'pass')

    def test_policy_binding_both_ways(self):
        good = capture(self.identity)
        # A journal image cannot be audited under another policy, or without a policy.
        for other in ('fw06-hello-v1', 'fw06-park-v1'):
            self.policy(other)
            with self.assertRaisesRegex(ValueError, 'Journal policy required'):
                self.audit(good)
        self.policy('fw06-hello-v1')
        with self.assertRaisesRegex(ValueError, 'Flash journal policy/artifact mismatch'):
            self.startup_audit(good)
        self.policy(None)
        with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
            self.startup_audit(good)
        self.policy('fw15-journal-v2')
        with self.assertRaisesRegex(ValueError, 'Unknown flash handshake policy'):
            self.startup_audit(good)
        # A hello-only or park image cannot be audited under the journal policy.
        self.policy('fw15-journal-v1')
        self.image(self.base)
        for call in (self.audit, self.startup_audit):
            with self.assertRaisesRegex(ValueError, 'Journal|journal'):
                call(good)
        self.image(self.base + b' ' + S.PARK_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash park policy/artifact mismatch'):
            self.startup_audit(good)
        # The journal policy still requires the HELLO record format.
        self.image(self.base.replace(S.FLASH_MARK, b'') + S.JOURNAL_MARK)
        for call in (self.audit, self.startup_audit):
            with self.assertRaisesRegex(ValueError, 'Flash handshake policy/artifact mismatch'):
                call(good)

    def test_other_policies_refuse_journal_records_and_keep_their_outcomes(self):
        # A park image carrying the journal start format is refused under the park policy.
        self.policy('fw06-park-v1')
        self.image(self.base + b' ' + S.PARK_MARK + b' ' + S.JOURNAL_MARK)
        with self.assertRaisesRegex(ValueError, 'Flash journal policy/artifact mismatch'):
            self.startup_audit(capture(self.identity))
        # A park image without the journal format keeps the park handshake semantics.
        self.image(self.base + b' ' + S.PARK_MARK)
        chunks, finished = capture(self.identity)
        boot3 = [(t, d.replace(b'PLJNL1 start mode=2 marker=00000002 watchdog=pass installed=1',
                               b'PLPRK1 start mode=2 marker=00000002 watchdog=pass'), b, k) for t, d, b, k in chunks if b == 3]
        self.assertEqual(self.startup_audit((boot3, finished))['outcome'], 'pass')


if __name__ == '__main__':
    unittest.main()
