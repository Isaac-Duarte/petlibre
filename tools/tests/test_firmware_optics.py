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


M = load('firmware_optics', TOOLS / 'audit/audit-firmware-optics.py')
J = load('firmware_wifi_join_tests_optics', TOOLS / 'tests/test_firmware_wifi_join.py')  # startup/LED capture helpers
S = M.STARTUP
LIT = {'off': (0, 0, 0), 'a': (1, 0, 0), 'b': (0, 1, 0), 'both': (1, 1, 0)}


def window(n, start, states=('unknown', 'unknown'), lit=None, edges=(), **over):
    emitters, length = M.plan(n)
    on = lit or LIT[emitters]
    samples = length // 2
    w = dict(n=n, emitters=emitters, start=start, len=length + 4, samples=samples, gap=3, edges=len(edges),
             dropped=0, food=states[0], outlet=states[1], stopped=1)
    w.update(over)
    first = ','.join('40' if v else '-' for v in on)
    line = (f"PLOPT1 window n={w['n']} emitters={w['emitters']} start={w['start']} len={w['len']} "
            f"samples={w['samples']} gap={w['gap']} edges={w['edges']} dropped={w['dropped']} "
            f"food={w['food']} outlet={w['outlet']} stopped={w['stopped']}\r\n")
    highs = ','.join(str(samples - 20 if v else 0) for v in on)
    bits = ''.join(str(v) for v in on)
    line += f"PLOPT1 inputs n={n} highs={highs} begin=000 end={bits} first={first} low_min=-,-,-\r\n"
    for e in edges:
        line += f"PLOPT1 edge n={n} input={e[0]} level={e[1]} ms={e[2]}\r\n"
    return line.encode(), start + w['len']


def run(states=None, lit=None, edits=None, stall=b'PLOPT1 stall cut=305 fault=7 off=1 stopped=1 closed=1\r\n',
        done=b'PLOPT1 done windows=37\r\n', count=M.WINDOWS):
    """(seconds, line) records after the LED probe."""
    out = [(0.1, b'PLOPT1 open board=1 ready=1 attached=1 events=1 inputs=1 ms=12\r\n')]
    start = 0
    for n in range(count):
        kw = dict((edits or {}).get(n, {}))
        line, start = window(n, start, states=(states or {}).get(n, ('unknown', 'unknown')),
                             lit=(lit or {}).get(n), **kw)
        out.append((0.2 + start / 1000, line))
    if stall:
        out.append((0.5 + start / 1000, stall))
    if done:
        out.append((0.6 + start / 1000, done))
    return out


def both_states():
    """Operator blocking seen through the filter in both windows."""
    both = [n for n in range(M.WINDOWS) if M.plan(n)[0] == 'both']
    return {both[0]: ('empty', 'clear'), both[1]: ('present', 'clear'), both[2]: ('empty', 'blocked')}


class FirmwareOptics(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.p = Path(self.tmp.name)
        self.manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
        self.identity = S.digest(json.dumps(self.manifest, sort_keys=True, separators=(',', ':')).encode())
        self.manifest.update(startup_identity=self.identity, startup_output_policy='stop-only', startup_led_policy='bounded-probe-v3',
                             startup_gpio_routing_policy='read-only-v2', startup_shared_pad_policy='before-km4-v3',
                             startup_gpio_clock_policy='explicit-enable-v1', startup_flash_handshake_policy=None,
                             startup_wifi_policy='fw07-dns-v1', startup_transport_policy='fw08-transport-v1',
                             startup_optics_policy='fw13-optics-v1')
        self.base = (f'PLFW1 start build={self.identity} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= '
                     'PLIO1 stage= share_pad= PLPWR1 before_func='.encode() + S.POWER_V3_FORMAT)
        self.image(self.base + b' ' + S.WIFI_DNS_MARK + b' ' + S.TRANSPORT_MARK + b' ' + S.OPTICS_MARK)

    def tearDown(self):
        self.tmp.cleanup()

    def image(self, image):
        self.manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': S.digest(image), 'bytes': len(image)}]
        (self.p / 'image.bin').write_bytes(image)
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))

    def audit(self, lines, stage, start=b'PLOPT1 start created=1\r\n'):
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

    def test_plan_matches_the_firmware(self):
        self.assertEqual(M.WINDOWS, 37)
        self.assertEqual([M.plan(n)[0] for n in range(8)], ['off', 'a', 'off', 'b', 'off', 'both', 'off', 'a'])

    def test_stage_b_reports_pairing_settling_and_the_stall(self):
        report = self.audit(run(), 'b')
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['optics_pairing'], {'off': [[]], 'a': [['food']], 'b': [['outlet']],
                                                    'both': [['food', 'outlet']]})
        self.assertEqual(report['optics_first_change_ms']['a:food'], {'min': 40, 'max': 40, 'count': 6})
        self.assertEqual(report['optics_stall']['cut_ms'], 305)
        self.assertEqual(report['optics_windows'], 37)
        # Unblocked, stage c lacks the operator's states.
        self.assertEqual(self.audit(run(), 'c')['outcome'], 'optics_unblocked')

    def test_stage_c_needs_both_states_of_each_input(self):
        report = self.audit(run(states=both_states(), lit={5: (0, 1, 0)}), 'c')
        self.assertEqual(report['outcome'], 'pass', report)
        self.assertEqual(report['optics_both_states'][1], {'n': 11, 'food': 'present', 'outlet': 'clear'})
        # Blocking makes the pairing vary, which stage b refuses.
        self.assertEqual(self.audit(run(states=both_states(), lit={5: (0, 1, 0)}), 'b')['outcome'], 'optics_inconsistent')
        states = both_states()
        states[17] = ('empty', 'clear')
        self.assertEqual(self.audit(run(states=states), 'c')['outcome'], 'optics_unblocked')

    def test_edges_must_match_their_counts(self):
        edges = [('outlet', 1, 41), ('outlet', 0, 900)]
        self.assertEqual(self.audit(run(edits={3: {'edges': edges}}), 'b')['outcome'], 'pass')
        many = [('feedback', i % 2, i) for i in range(16)]
        self.assertEqual(self.audit(run(edits={3: {'edges': many, 'dropped': 0}}), 'b')['outcome'], 'pass')
        # 20 counted, 16 kept: 4 dropped, or the record is inconsistent.
        lines = run(edits={3: {'edges': many}})
        lines = [(t, d.replace(b'edges=16 dropped=0', b'edges=20 dropped=4')) for t, d in lines]
        self.assertEqual(self.audit(lines, 'b')['outcome'], 'pass')
        lines = [(t, d.replace(b'edges=20 dropped=4', b'edges=20 dropped=3')) for t, d in lines]
        self.assertEqual(self.audit(lines, 'b')['outcome'], 'optics_sequence')

    def test_failures_have_distinct_outcomes(self):
        open_line = b'PLOPT1 open board=1 ready=1 attached=1 events=1 inputs=1 ms=12\r\n'
        cases = [
            (run(count=20, stall=None, done=None), 'optics_incomplete'),
            (run(stall=None, done=None), 'optics_incomplete'),
            (run(edits={4: {'stopped': 0}}), 'optics_stopped'),
            (run(edits={4: {'gap': 101}}), 'optics_gap'),
            (run(edits={4: {'len': 4200}}), 'optics_window'),
            (run(edits={4: {'len': 3990}}), 'optics_window'),
            (run(edits={4: {'emitters': 'both'}}), 'optics_sequence'),
            (run(stall=b'PLOPT1 stall cut=none fault=0 off=0 stopped=1 closed=0\r\n'), 'optics_stall'),
            (run(stall=b'PLOPT1 stall cut=290 fault=7 off=1 stopped=1 closed=1\r\n'), 'optics_stall'),
            (run(stall=b'PLOPT1 stall cut=350 fault=7 off=1 stopped=1 closed=1\r\n'), 'optics_stall'),
            (run(stall=b'PLOPT1 stall cut=305 fault=8 off=1 stopped=1 closed=1\r\n'), 'optics_stall'),
            (run(done=b'PLOPT1 done windows=36\r\n'), 'optics_sequence'),
            (run(lit={7: (0, 1, 0)}), 'optics_inconsistent'),
            ([(0.1, b'PLOPT1 timeout ms=2000\r\n')], 'optics_timeout'),
            ([(0.1, open_line.replace(b'ready=1', b'ready=0'))], 'optics_open'),
            (run(count=3, stall=None, done=None) + [(9, b'PLOPT1 error kind=supervisor n=3 fault=7\r\n')], 'optics_error'),
            (run() + [(300, b'PLOPT1 window n=37 emitters=off\r\n')], 'optics_sequence'),
        ]
        for index, (lines, outcome) in enumerate(cases):
            with self.subTest(index=index, outcome=outcome):
                self.assertEqual(self.audit(lines, 'b')['outcome'], outcome)
        self.assertEqual(self.audit(run(), 'b', start=b'PLOPT1 start created=0\r\n')['outcome'], 'optics_start')
        # Shifted window starts break contiguity.
        lines = [(t, d.replace(b'n=6 emitters=off start=', b'n=6 emitters=off start=1')) for t, d in run()]
        self.assertEqual(self.audit(lines, 'b')['outcome'], 'optics_sequence')

    def test_policy_binding_both_ways(self):
        self.manifest['startup_optics_policy'] = None
        (self.p / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, 'Optics policy required'):
            self.audit(run(), 'b')
        self.manifest['startup_optics_policy'] = 'fw13-optics-v1'
        self.image(self.base + b' ' + S.WIFI_DNS_MARK + b' ' + S.TRANSPORT_MARK)
        with self.assertRaisesRegex(ValueError, 'Optics policy/artifact mismatch'):
            self.audit(run(), 'b')


if __name__ == '__main__':
    unittest.main()
