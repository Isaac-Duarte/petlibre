import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('firmware_startup', Path(__file__).resolve().parents[1] / 'audit-firmware-startup.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
ID = 'a' * 64


def start(identity=ID, bank=b'1', control=b'00000003', offset=b'03e7a000'):
    return b'PLFW1 start build=' + identity.encode() + b' bank=' + bank + b' reset=00100000 mmu=' + control + b',0c000000,' + offset + b'\r\n'


def boot(identity=ID):
    return b'Petlibro local firmware: Rust bringup\r\n' + start(identity) + b'Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n' + M.PASS


class FirmwareStartup(unittest.TestCase):
    def test_routing_observations_are_fixed_fields_and_complete_sequence(self):
        stages = ['startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22']
        records = [f'PLIO1 stage={stage} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000\r\n'.encode() for stage in stages]
        full = b''.join(records)
        report = M.classify_routing(full)
        self.assertTrue(report['gpio_routing_complete'])
        self.assertEqual(len(report['gpio_routing_records']), 9)
        self.assertEqual(report['gpio_routing_records'][0]['groups'], '0000ffff')
        for bad in [full[:-len(records[-1])], full + records[-1], full.replace(b'stage=PA0', b'stage=PB22'), full.replace(b'aon=00020003', b'aon=untrusted')]:
            self.assertFalse(M.classify_routing(bad)['gpio_routing_complete'])
        self.assertNotIn('untrusted', repr(M.classify_routing(full.replace(b'aon=00020003', b'aon=untrusted'))))

    def test_routing_v2_adds_supply_and_raw_port_fields(self):
        stages = ['startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22']
        records = [f'PLIO1 stage={stage} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001\r\n'.encode() for stage in stages]
        report = M.classify_routing(b''.join(records), 2)
        self.assertTrue(report['gpio_routing_complete'])
        self.assertEqual(report['gpio_routing_records'][1]['share_pad'], '80000355')
        self.assertEqual(report['gpio_routing_records'][1]['ext_a'], '00006001')
        self.assertFalse(M.classify_routing(b''.join(records), 1)['gpio_routing_complete'])

    def test_readback_v2_reports_samples_without_changing_outcome(self):
        detail = b'PLDBG1 pin=PA0 stage=verify want=1 latch=1 input=0 highs=17/64 directions=1 stopped=1 pad=00005c00\r\n'
        failed = M.classify_led(b'PLLED1 begin\r\n' + detail + b'PLLED1 error step=unverified\r\n', 2)
        self.assertEqual(failed['outcome'], 'led_failed')
        self.assertEqual(failed['led_readbacks'][0]['input_high_samples'], 17)
        self.assertFalse(failed['led_readbacks'][0]['input_high'])
        for bad in (b'highs=65/64', b'highs=17/32', b'highs=017/64'):
            self.assertEqual(M.classify_led(detail.replace(b'highs=17/64', bad), 2)['led_readbacks'], [])
        self.assertEqual(M.classify_led(detail, 1)['led_readbacks'], [])

    def test_shared_pad_record_must_be_intact_single_preserving_and_before_km4(self):
        line = b'PLPWR1 before_func=0201019a before_pad=80000000 after_func=1201019a after_pad=80000355\n\r'
        clock = b'PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n'
        report = M.classify_power(line + clock)
        self.assertTrue(report['shared_pad_verified'])
        self.assertEqual(report['shared_pad_control_after'], '80000355')
        for rx, outcome in [(clock, 'shared_pad_missing'), (clock + line, 'shared_pad_order'),
                            (line * 2 + clock, 'shared_pad_malformed'),
                            (line[:40] + clock[:20] + line[40:] + clock[20:], 'shared_pad_malformed'),
                            (line.replace(b'after_func=1201019a', b'after_func=0201019a') + clock, 'shared_pad_failed'),
                            (line.replace(b'after_func=1201019a', b'after_func=1201019b') + clock, 'shared_pad_failed'),
                            (line.replace(b'after_pad=80000355', b'after_pad=80000155') + clock, 'shared_pad_failed'),
                            (line.replace(b'after_pad=80000355', b'after_pad=c0000355') + clock, 'shared_pad_failed')]:
            with self.subTest(outcome=outcome, rx=rx):
                self.assertEqual(M.classify_power(rx)['outcome'], outcome)

    def test_shared_pad_v2_requires_stock_ldo_trim(self):
        clock = b'PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n'
        line = b'PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=%s\n\r'
        for pad, v1, v2 in [(b'00000301', 'pass', 'shared_pad_failed'), (b'00000355', 'pass', 'pass'),
                            (b'000003d1', 'pass', 'pass'), (b'00000353', 'pass', 'shared_pad_failed')]:
            with self.subTest(pad=pad):
                self.assertEqual(M.classify_power(line % pad + clock, 1)['outcome'], v1)
                self.assertEqual(M.classify_power(line % pad + clock, 2)['outcome'], v2)
        self.assertEqual(M.classify_power(line % b'00000355' + clock, 2)['shared_pad_ldo_trim'], '2a')

    def test_shared_pad_v3_reports_port_a_samples(self):
        clock = b'PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n'
        line = b'PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r'
        report = M.classify_power(line + clock, 3)
        self.assertEqual(report['outcome'], 'pass')
        self.assertEqual(report['shared_pad_port_a_samples'], ['e1c46e80', 'e1c46e00', 'e1c46e00', 'e1c46e00'])
        self.assertEqual(M.classify_power(line.replace(b'00000355', b'00000301') + clock, 3)['outcome'], 'shared_pad_failed')
        self.assertEqual(M.classify_power(line.replace(b',e1c46e00\n', b'\n') + clock, 3)['outcome'], 'shared_pad_malformed')
        self.assertEqual(M.classify_power(line + clock, 2)['outcome'], 'shared_pad_malformed')

    def test_clock_bits_and_preservation(self):
        line = b'PLCLK1 function_before=00000041 clock_before=00000082 function_after=00100041 clock_after=00100082 status=pass\r\n'
        self.assertTrue(M.classify_clock(line)['gpio_clock_verified'])
        for changed in (line.replace(b'00100041', b'00000041'), line.replace(b'00100082', b'00000082'), line.replace(b'00100082', b'00100083'), line.replace(b'status=pass', b'status=fail')):
            self.assertEqual(M.classify_clock(changed)['outcome'], 'gpio_clock_failed')
        self.assertEqual(M.classify_clock(b'')['outcome'], 'gpio_clock_missing')
        self.assertEqual(M.classify_clock(line * 2)['outcome'], 'gpio_clock_repeated')
    def test_clock_policy_binding_and_order(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            manifest = {'profile': 'plaf103-firstboot', 'inputs': {}}
            identity = M.digest(json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode())
            manifest.update(startup_identity=identity, startup_output_policy='stop-only', startup_gpio_clock_policy='explicit-enable-v1')
            image = f'PLFW1 start build={identity} PLFW1 outputs pa= PLCLK1 function_before='.encode()
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(image), 'bytes': len(image)}]
            (p / 'manifest.json').write_text(json.dumps(manifest))
            (p / 'image.bin').write_bytes(image)
            (p / 'serial-tx.bin').write_bytes(b'')
            clock = b'PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n'
            outputs = b'PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n'
            for rx, outcome in [(clock + outputs + boot(identity), 'pass'), (outputs + clock + boot(identity), 'gpio_clock_order'), (outputs + boot(identity), 'gpio_clock_missing')]:
                (p / 'serial-rx.bin').write_bytes(rx)
                metadata = {'mode': 'observe', 'status': 'complete', 'end_utc': '2026-10-20T10:02:00+00:00', 'serial-rx.bin_sha256': M.digest(rx), 'serial-tx.bin_sha256': M.digest(b'')}
                (p / 'report.json').write_text(json.dumps(metadata))
                (p / 'serial-events.jsonl').write_text(json.dumps({'direction': 'rx', 'offset': 0, 'bytes': len(rx), 'utc': '2026-10-20T10:00:00+00:00'}) + '\n')
                self.assertEqual(M.verify(p, p / 'manifest.json', p / 'image.bin', 60)['outcome'], outcome)
            routed_image = image + b'PLIO1 stage='
            manifest['startup_gpio_routing_policy'] = 'read-only-v1'
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(routed_image), 'bytes': len(routed_image)}]
            (p / 'manifest.json').write_text(json.dumps(manifest))
            (p / 'image.bin').write_bytes(routed_image)
            stages = ['startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22']
            records = [f'PLIO1 stage={stage} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000\r\n'.encode() for stage in stages]
            for snapshots, outcome in [(b''.join(records), 'pass'), (b''.join(records[:-1]), 'gpio_routing_incomplete')]:
                rx = clock + snapshots + outputs + boot(identity)
                (p / 'serial-rx.bin').write_bytes(rx)
                metadata['serial-rx.bin_sha256'] = M.digest(rx)
                (p / 'report.json').write_text(json.dumps(metadata))
                (p / 'serial-events.jsonl').write_text(json.dumps({'direction': 'rx', 'offset': 0, 'bytes': len(rx), 'utc': '2026-10-20T10:00:00+00:00'}) + '\n')
                report = M.verify(p, p / 'manifest.json', p / 'image.bin', 60)
                self.assertEqual(report['outcome'], outcome)
                self.assertEqual(len(report['gpio_routing_records']), snapshots.count(b'PLIO1'))
            manifest.pop('startup_gpio_routing_policy')
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): M.verify(p, p / 'manifest.json', p / 'image.bin', 60)
            manifest['startup_gpio_routing_policy'] = 'read-only-v2'
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): M.verify(p, p / 'manifest.json', p / 'image.bin', 60)
            manifest['startup_gpio_routing_policy'] = 'read-only-v1'
            manifest['startup_shared_pad_policy'] = 'before-km4-v1'
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): M.verify(p, p / 'manifest.json', p / 'image.bin', 60)
            manifest.pop('startup_shared_pad_policy')
            power_image = routed_image + b'PLPWR1 before_func=%08x before_pad=%08x after_func=%08x ' + M.POWER_V3_FORMAT
            power = b'PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r'
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(power_image), 'bytes': len(power_image)}]
            (p / 'image.bin').write_bytes(power_image)
            rx = power + clock + b''.join(records) + outputs + boot(identity)
            (p / 'serial-rx.bin').write_bytes(rx)
            metadata['serial-rx.bin_sha256'] = M.digest(rx)
            (p / 'report.json').write_text(json.dumps(metadata))
            (p / 'serial-events.jsonl').write_text(json.dumps({'direction': 'rx', 'offset': 0, 'bytes': len(rx), 'utc': '2026-10-20T10:00:00+00:00'}) + '\n')
            manifest['startup_shared_pad_policy'] = 'before-km4-v3'
            (p / 'manifest.json').write_text(json.dumps(manifest))
            report = M.verify(p, p / 'manifest.json', p / 'image.bin', 60)
            self.assertEqual(report['outcome'], 'pass')
            self.assertTrue(report['shared_pad_verified'])
            # Downgrades: omitting the policy, or declaring an older one, is refused for a v3 image.
            for policy in (None, 'before-km4-v1', 'before-km4-v2'):
                with self.subTest(policy=policy):
                    if policy:
                        manifest['startup_shared_pad_policy'] = policy
                    else:
                        manifest.pop('startup_shared_pad_policy')
                    (p / 'manifest.json').write_text(json.dumps(manifest))
                    with self.assertRaisesRegex(ValueError, 'Shared-pad v3'): M.verify(p, p / 'manifest.json', p / 'image.bin', 60)
            manifest.pop('startup_shared_pad_policy', None)
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(routed_image), 'bytes': len(routed_image)}]
            (p / 'image.bin').write_bytes(routed_image)
            manifest.pop('startup_gpio_clock_policy')
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): M.verify(p, p / 'manifest.json', p / 'image.bin', 60)

    def test_led_sequence_and_terminal_failures(self):
        records = [b'PLLED1 begin\r\n']
        for pin in (b'PA0', b'PA2', b'PA4', b'PB22'):
            records += [b'PLLED1 pin=' + pin + b' level=' + n + b' status=pass\r\n' for n in (b'1', b'0', b'1', b'0')]
        records += [b'PLLED1 done low=all\r\n']
        full = b''.join(records)
        self.assertTrue(M.classify_led(full)['led_probe_verified'])
        self.assertEqual(M.classify_led(full)['led_input_verified_pins'], ['PA0', 'PA2', 'PA4', 'PB22'])
        self.assertEqual(M.classify_led(b''.join(records[:-1]))['led_input_verified_pins'], [])
        self.assertEqual(M.classify_led(full.replace(b'status=pass', b'status=fail', 1))['outcome'], 'led_failed')
        self.assertEqual(M.classify_led(full + b'PLLED1 error finish=input_mismatch low=all\r\n')['outcome'], 'led_failed')
        detail = b'PLDBG1 pin=PA0 stage=verify want=1 latch=1 input=0 directions=1 stopped=1 pad=00000000\r\n'
        failed = M.classify_led(records[0] + detail + b'PLLED1 error step=unverified\r\n')
        self.assertEqual(failed['outcome'], 'led_failed')
        self.assertEqual(failed['led_readbacks'][0], {'pin': 'PA0', 'stage': 'verify', 'want_high': True, 'latch_high': True, 'input_high': False, 'directions_ok': True, 'stopped_ok': True, 'pad_control': '00000000'})
        for rx, outcome in [(b'', 'led_missing'), (b''.join(records[:-1]), 'led_incomplete'), (full + records[-1], 'led_unexpected_sequence'), (full.replace(b'pin=PA2', b'pin=PA3'), 'led_unexpected_sequence'), (full + b'PLLED1 error step=unverified\r\n', 'led_failed'), (full[:-2], 'led_unexpected_sequence')]:
            with self.subTest(outcome=outcome): self.assertEqual(M.classify_led(rx)['outcome'], outcome)

    def test_led_v3_labels_port_a_output_only_and_never_passes_them(self):
        records = [b'PLLED1 begin output_only=PA0,PA2,PA4\r\n']
        for pin, status in ((b'PA0', b'output'), (b'PA2', b'output'), (b'PA4', b'output'), (b'PB22', b'pass')):
            records += [b'PLLED1 pin=' + pin + b' level=' + n + b' status=' + status + b'\r\n' for n in (b'1', b'0', b'1', b'0')]
        records += [b'PLLED1 done low=all\r\n']
        full = b''.join(records)
        report = M.classify_led(full, 3)
        self.assertEqual(report['outcome'], 'pass')
        self.assertEqual(report['led_output_only_pins'], ['PA0', 'PA2', 'PA4'])
        # FW04 accepts the sequence, but only PB22 was input-verified.
        self.assertFalse(report['led_probe_verified'])
        self.assertEqual(report['led_input_verified_pins'], ['PB22'])
        for bad, outcome in [(full.replace(b'pin=PA0 level=1 status=output', b'pin=PA0 level=1 status=pass'), 'led_unexpected_sequence'),
                             (full.replace(b'pin=PB22 level=1 status=pass', b'pin=PB22 level=1 status=output', 1), 'led_unexpected_sequence'),
                             (full.replace(b' output_only=PA0,PA2,PA4', b''), 'led_unexpected_sequence'),
                             (full.replace(b'pin=PB22 level=1 status=pass', b'pin=PB22 level=1 status=fail', 1), 'led_failed')]:
            with self.subTest(outcome=outcome): self.assertEqual(M.classify_led(bad, 3)['outcome'], outcome)
        self.assertNotEqual(M.classify_led(full, 2)['outcome'], 'pass')

    def test_stopped_profile_requires_correct_levels_directions_and_order(self):
        outputs = b'PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n'
        self.assertEqual(M.classify(outputs + boot(), ID, True)['outcome'], 'pass')
        self.assertTrue(M.classify(outputs + boot(), ID, True)['stopped_gpio_verified'])
        self.assertEqual(M.classify((outputs + boot()) * 2, ID, True)['outcome'], 'repeated_startup')
        self.assertEqual(M.classify(boot(), ID, True)['outcome'], 'outputs_missing')
        self.assertEqual(M.classify(boot() + outputs, ID, True)['outcome'], 'outputs_after_startup')
        for old, new in [(b'pa=00006000', b'pa=00002000'), (b'pb=00000000', b'pb=00000008'), (b'dira=0000e000', b'dira=00006000'), (b'dirb=00000008', b'dirb=00000000'), (b'status=pass', b'status=fail')]:
            with self.subTest(field=old):
                self.assertEqual(M.classify(outputs.replace(old, new) + boot(), ID, True)['outcome'], 'outputs_failed')

    def test_classification(self):
        for rx, expected in [(boot(), 'pass'), (b'', 'timeout'), (start(), 'incomplete_report'),
                             (boot('b' * 64), 'wrong_build'), (boot() * 2, 'repeated_startup'),
                             (boot() + b'HardFault', 'fault'), (boot() + M.FAIL, 'diagnostic_failed'),
                             (boot().replace(b'bank=1', b'bank=unknown'), 'unknown_mapping'),
                             (boot().replace(b'03e7a000', b'ffffffff'), 'unknown_mapping'),
                             (boot().replace(b'00000003,', b'00000000,'), 'unknown_mapping'),
                             (boot().replace(b'PLFW1 result ', b'PLFW1 invalid '), 'malformed_report'),
                             (M.PASS + start(), 'incomplete_report'), (boot() + M.PASS, 'malformed_report')]:
            with self.subTest(expected=expected, rx=rx):
                self.assertEqual(M.classify(rx, ID)['outcome'], expected)
        self.assertEqual(M.classify(boot(), ID)['reset_flags'], '00100000')
        self.assertEqual(M.classify(boot().replace(b'bank=1', b'bank=0').replace(b'03e7a000', b'03ffa000'), ID)['outcome'], 'pass')

    def test_evidence_artifact_binding_and_window(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            manifest = {'profile': 'plaf103-firstboot', 'inputs': {'source': 'f' * 64}}
            identity = M.digest(json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode())
            manifest['startup_identity'] = identity
            image = b'container ' + f'PLFW1 start build={identity} '.encode()
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(image), 'bytes': len(image)}]
            (p / 'manifest.json').write_text(json.dumps(manifest)); (p / 'image.bin').write_bytes(image)
            rx = boot(identity)
            (p / 'serial-rx.bin').write_bytes(rx); (p / 'serial-tx.bin').write_bytes(b'')
            metadata = {'mode': 'observe', 'status': 'complete', 'end_utc': '2026-10-20T10:02:00+00:00', 'serial-rx.bin_sha256': M.digest(rx), 'serial-tx.bin_sha256': M.digest(b'')}
            (p / 'report.json').write_text(json.dumps(metadata))
            event = {'direction': 'rx', 'offset': 0, 'bytes': len(rx), 'utc': '2026-10-20T10:00:00+00:00'}
            (p / 'serial-events.jsonl').write_text(json.dumps(event) + '\n')
            call = lambda minimum=60: M.verify(p, p / 'manifest.json', p / 'image.bin', minimum)
            self.assertEqual(call()['outcome'], 'pass')
            self.assertEqual(call(180)['outcome'], 'observation_too_short')
            manifest['startup_gpio_routing_policy'] = 'read-only-v1'
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): call()
            manifest.pop('startup_gpio_routing_policy')
            manifest['startup_output_policy'] = 'stop-only'
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): call()
            manifest.pop('startup_output_policy')
            (p / 'manifest.json').write_text(json.dumps(manifest))
            manifest['startup_led_policy'] = 'bounded-probe-v1'
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): call()
            manifest.pop('startup_led_policy')
            (p / 'manifest.json').write_text(json.dumps(manifest))
            sequence = b'PLLED1 begin\r\n'
            for pin in (b'PA0', b'PA2', b'PA4', b'PB22'):
                sequence += b''.join(b'PLLED1 pin=' + pin + b' level=' + n + b' status=pass\r\n' for n in (b'1', b'0', b'1', b'0'))
            sequence += b'PLLED1 done low=all\r\n'
            led_image = image + b'PLLED1 begin'
            manifest['startup_led_policy'] = 'bounded-probe-v1'
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(led_image), 'bytes': len(led_image)}]
            (p / 'manifest.json').write_text(json.dumps(manifest)); (p / 'image.bin').write_bytes(led_image)
            for received, outcome in [(rx + sequence, 'pass'), (sequence + rx, 'led_unexpected_sequence'), (rx + sequence[:-len(b'PLLED1 done low=all\r\n')], 'led_incomplete')]:
                (p / 'serial-rx.bin').write_bytes(received)
                metadata['serial-rx.bin_sha256'] = M.digest(received)
                (p / 'report.json').write_text(json.dumps(metadata))
                event['bytes'] = len(received)
                (p / 'serial-events.jsonl').write_text(json.dumps(event) + '\n')
                self.assertEqual(call()['outcome'], outcome)
            manifest.pop('startup_led_policy')
            manifest['artifacts'] = [{'path': 'OTA_All.bin', 'sha256': M.digest(image), 'bytes': len(image)}]
            (p / 'manifest.json').write_text(json.dumps(manifest)); (p / 'image.bin').write_bytes(image)
            (p / 'serial-rx.bin').write_bytes(rx)
            metadata['serial-rx.bin_sha256'] = M.digest(rx)
            (p / 'report.json').write_text(json.dumps(metadata))
            event['bytes'] = len(rx)
            event['offset'] = 1
            (p / 'serial-events.jsonl').write_text(json.dumps(event) + '\n')
            with self.assertRaises(ValueError): call()
            event['offset'] = 0
            (p / 'serial-events.jsonl').write_text(json.dumps(event) + '\n')
            (p / 'image.bin').write_bytes(image + b'corrupt')
            with self.assertRaises(ValueError): call()
            (p / 'image.bin').write_bytes(image)
            manifest['inputs']['source'] = 'e' * 64
            (p / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError): call()
