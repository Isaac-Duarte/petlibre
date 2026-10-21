#!/usr/bin/env python3
"""Verify finalized receive-only diagnostic startup against its build artifact."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('startup_evidence', ROOT / 'tools/audit-uart-startups.py')
EVIDENCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVIDENCE)
START = re.compile(rb'PLFW1 start build=([0-9a-f]{64}) bank=(0|1|unknown) reset=([0-9a-f]{8}) mmu=([0-9a-f]{8}),([0-9a-f]{8}),([0-9a-f]{8})\r\n')
PASS = b'PLFW1 result diagnostic=pass backend=unopened\r\n'
FAIL = b'PLFW1 result diagnostic=fail backend=unopened\r\n'
OUTPUTS = re.compile(rb'PLFW1 outputs pa=([0-9a-f]{8}) pb=([0-9a-f]{8}) dira=([0-9a-f]{8}) dirb=([0-9a-f]{8}) status=(pass|fail)\r\n')
POWER = {1: re.compile(rb'PLPWR1 before_func=([0-9a-f]{8}) before_pad=([0-9a-f]{8}) after_func=([0-9a-f]{8}) after_pad=([0-9a-f]{8})()\n\r'),
         3: re.compile(rb'PLPWR1 before_func=([0-9a-f]{8}) before_pad=([0-9a-f]{8}) after_func=([0-9a-f]{8}) after_pad=([0-9a-f]{8}) ext_a=((?:[0-9a-f]{8},){3}[0-9a-f]{8})\n\r')}
ROUTING = {1: rb'PLIO1 stage=(startup|PA0|PA2|PA4|PB22) groups=([0-9a-f]{8}) aon=([0-9a-f]{8}) control_a=([0-9a-f]{8}) control_b=([0-9a-f]{8})\r\n',
           2: rb'PLIO1 stage=(startup|PA0|PA2|PA4|PB22) groups=([0-9a-f]{8}) aon=([0-9a-f]{8}) control_a=([0-9a-f]{8}) control_b=([0-9a-f]{8}) share_pad=([0-9a-f]{8}) ext_a=([0-9a-f]{8})\r\n'}
ROUTING_FIELDS = ('stage', 'groups', 'aon', 'control_a', 'control_b', 'share_pad', 'ext_a')
READBACK = {1: rb'PLDBG1 pin=(PA0|PA2|PA4|PB22) stage=(request|verify|initialize) want=([01]) latch=([01]) input=([01])() directions=([01]) stopped=([01]) pad=([0-9a-f]{8})\r\n',
            2: rb'PLDBG1 pin=(PA0|PA2|PA4|PB22) stage=(request|verify|initialize) want=([01]) latch=([01]) input=([01]) highs=([0-9]|[1-5][0-9]|6[0-4])/64 directions=([01]) stopped=([01]) pad=([0-9a-f]{8})\r\n'}
AMACRO = 1 << 28
SHARE_PAD_HELPER = 0x3ff  # PORB, MBIAS and LDO trim/power, per the stock trace
SUPERVISION_MARK = b'PLTMR1 open board='
FLASH_MARK = b'PLFLS1 hello authority='
PARK_MARK = b'PLPRK1 start mode='  # FW06 stage C park round trip follows READY
READ_MARK = b'PLRDS1 start marker='  # FW06 stage D mediated flash read follows READY
READ = {'start': re.compile(rb'PLRDS1 start marker=([0-9a-f]{8}) watchdog=(pass|fail) installed=([01])\r\n'),
        'batch': re.compile(rb'PLRDS1 batch reads=([0-9]{1,10}) id=([0-9a-f]{6}) mismatches=([0-9]{1,10}) park_max=([0-9]{1,10}) '
                            rb'release_max=([0-9]{1,10}) frozen=([0-9]{1,10}) alive=([0-9]{1,10})\r\n'),
        'done': re.compile(rb'PLRDS1 done reads=([0-9]{1,10}) leases=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10})\r\n'),
        'skip': re.compile(rb'PLRDS1 skip marker=([0-9a-f]{8})\r\n')}
READ_BATCHES = (25, 50, 75, 100)
READ_ID = 'c84017'  # JEDEC ID of the research unit's GigaDevice flash (boot log `Flash ID:c8, 40, 17`)
READ_DONE = (100, 100, 1, 0)  # reads, leases, phase IDLE, no fault
EXCHANGE_TICKS = 1310  # 40 ms park/release exchange limit at 32 768 Hz
WRITE_MARK = b'PLWRS1 start marker='  # FW06 stage E owner-approved scratch-sector write follows READY
DEC = rb'([0-9]{1,10})'
JOURNAL_MARK = b'PLJNL1 start mode='  # FW15 flash journal on NOR follows READY
SETTINGS_MARK = b'PLSET1 start marker='  # FW15 settings-store power-cut stress follows READY
ADC_MARK = b'PLADC1 open outputs='  # FW14 read-only power-input samples follow the LED probe
ADC_SUPPLY_MARK = b' supply='  # fw14-adc-v2 samples add internal channel 10
WRITE = {'start': re.compile(rb'PLWRS1 start marker=([0-9a-f]{8}) watchdog=(pass|fail) installed=([01])\r\n'),
         'preimage': re.compile(rb'PLWRS1 preimage id=([0-9a-f]{6}) blank=' + DEC + rb'\r\n'),
         'cycle': re.compile(rb'PLWRS1 cycle n=' + DEC + rb' erase_ticks=' + DEC + rb' blank=' + DEC + rb' program_max=' + DEC
                             + rb' errors=' + DEC + rb'\r\n'),
         'restore': re.compile(rb'PLWRS1 restore erase_ticks=' + DEC + rb' blank=' + DEC + rb'\r\n'),
         'done': re.compile(rb'PLWRS1 done cycles=' + DEC + rb' leases=' + DEC + rb' phase=' + DEC + rb' fault=' + DEC
                            + rb' park_max=' + DEC + rb' release_max=' + DEC + rb' frozen=' + DEC + rb'\r\n'),
         'skip': re.compile(rb'PLWRS1 skip marker=([0-9a-f]{8})\r\n')}
WRITE_CYCLES = (1, 2, 3)
WRITE_BLANK = 1024  # every sampled word of the 4 KiB scratch sector reads erased
WRITE_DONE = (3, 53, 1, 0)  # cycles, leases, phase IDLE, no fault; frozen equals leases
TICK_HZ = 32768
ERASE_TICKS = 16384  # 500 ms lease active budget
PROGRAM_TICKS = 1638  # 50 ms
FLASH = {'hello': re.compile(rb'PLFLS1 hello authority=([0-9a-f]{16}) count=([0-9]{1,10})\r\n'),
         'foreign': re.compile(rb'PLFLS1 foreign\r\n'),
         'ready': re.compile(rb'PLFLS1 ready peer=([0-9a-f]{16}) reply=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10}) inactive=([01])\r\n'),
         'peer': re.compile(rb'PLFLS1 peer peer=([0-9a-f]{16}) reply=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10}) blocked=([01])\r\n'),
         'done': re.compile(rb'PLFLS1 done phase=([0-9]{1,10}) fault=([0-9]{1,10}) blocked=([01])\r\n'),
         'error': re.compile(rb'PLFLS1 error (?:timeout ms=[0-9]{1,10}|clock)\r\n')}
# reply ACCEPTED/FAILED, phase IDLE/FAULT, fault PEER_LOST; the test KM0 republishes its session ~10 s after READY.
FLASH_READY = (0, 1, 0, 1)
FLASH_PEER = (5, 8, 7, 1)
FLASH_DONE = (8, 7, 1)
CLOCK = re.compile(rb'PLCLK1 function_before=([0-9a-f]{8}) clock_before=([0-9a-f]{8}) function_after=([0-9a-f]{8}) clock_after=([0-9a-f]{8}) status=(pass|fail)\r\n')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def classify_clock(rx):
    values = list(CLOCK.finditer(rx))
    if len(values) != 1 or rx.count(b'PLCLK1') != 1:
        return {'outcome': 'gpio_clock_missing' if not values else 'gpio_clock_repeated', 'gpio_clock_verified': False}
    m = values[0]
    before_function, before_clock, function, clock = [int(v, 16) for v in m.groups()[:4]]
    bit = 1 << 20
    ok = bool(function & bit and clock & bit) and function & ~bit == before_function & ~bit and clock & ~bit == before_clock & ~bit and m[5] == b'pass'
    return {'outcome': 'pass' if ok else 'gpio_clock_failed', 'gpio_clock_verified': ok,
            'gpio_function_before': m[1].decode(), 'gpio_clock_before': m[2].decode(),
            'gpio_function_after': m[3].decode(), 'gpio_clock_after': m[4].decode()}


def classify_power(rx, version=1):
    """One intact KM0 supply record, preserving unrelated bits, before any KM4 report.

    Version 2 also requires the stock CODEC_Init LDO trim (0x2A or 0x68); version 3
    adds read-only raw Port A samples after each supply step."""
    values = list(POWER[3 if version >= 3 else 1].finditer(rx))
    if len(values) != 1 or rx.count(b'PLPWR1') != 1:
        return {'outcome': 'shared_pad_missing' if b'PLPWR1' not in rx else 'shared_pad_malformed', 'shared_pad_verified': False}
    m = values[0]
    before_func, before_pad, func, pad = [int(v, 16) for v in m.groups()[:4]]
    km4 = [rx.find(marker) for marker in (b'PLCLK1', b'PLIO1', b'PLFW1') if marker in rx]
    ok = (func == before_func | AMACRO and pad & 0x301 == 0x301
          and pad & ~SHARE_PAD_HELPER == before_pad & ~SHARE_PAD_HELPER
          and (version < 2 or (pad >> 1) & 0x7f in (0x2A, 0x68)))
    outcome = 'shared_pad_failed' if not ok else 'shared_pad_order' if km4 and min(km4) < m.end() else 'pass'
    return {'outcome': outcome, 'shared_pad_verified': outcome == 'pass', 'shared_pad_ldo_trim': f'{(pad >> 1) & 0x7f:02x}',
            **dict(zip(('shared_pad_function_before', 'shared_pad_control_before', 'shared_pad_function_after', 'shared_pad_control_after'),
                       (v.decode() for v in m.groups()[:4]))),
            **({'shared_pad_port_a_samples': m[5].decode().split(',')} if version >= 3 else {})}


def classify_routing(rx, version=1):
    """Keep fixed numeric observations without inferring pin-group membership."""
    records = []
    for m in re.finditer(ROUTING[version], rx):
        records.append(dict(zip(ROUTING_FIELDS, (v.decode() for v in m.groups()))))
    stages = [r['stage'] for r in records]
    expected = ['startup', 'PA0', 'PA0', 'PA2', 'PA2', 'PA4', 'PA4', 'PB22', 'PB22']
    complete = stages == expected and rx.count(b'PLIO1') == len(records)
    return {'outcome': 'pass' if complete else 'gpio_routing_incomplete',
            'gpio_routing_complete': complete, 'gpio_routing_records': records}


OUTPUT_ONLY = (b'PA0', b'PA2', b'PA4')
LED_PINS = (b'PA0', b'PA2', b'PA4', b'PB22')
# The KM0 printf format of the before-KM4 (v3) shared-pad record, as linked.
POWER_V3_FORMAT = b'after_pad=%08x ext_a=%08x,%08x,%08x,%08x'


def classify_led(rx, version=1):
    """Independent expected bounded GPIO sequence; no installer/firmware imports.

    Version 3 labels PA0/PA2/PA4 output-only (no input readback); they report
    status=output, never pass. The sequence outcome can still be 'pass', but
    `led_probe_verified` means every pin was input-verified, so it stays False
    whenever output-only pins are present; `led_input_verified_pins` lists the
    pins whose levels were read back.
    """
    output_only = OUTPUT_ONLY if version >= 3 else ()
    expected = [b'PLLED1 begin output_only=' + b','.join(OUTPUT_ONLY) + b'\r\n' if output_only else b'PLLED1 begin\r\n']
    for pin in (b'PA0', b'PA2', b'PA4', b'PB22'):
        status = b'output' if pin in output_only else b'pass'
        for level in (b'1', b'0', b'1', b'0'):
            expected.append(b'PLLED1 pin=' + pin + b' level=' + level + b' status=' + status + b'\r\n')
    expected.append(b'PLLED1 done low=all\r\n')
    actual = re.findall(rb'PLLED1[^\r\n]*\r\n', rx)
    if b'PLLED1 error ' in rx or re.search(rb'PLLED1 pin=[^\r\n]* status=fail\r\n', rx):
        outcome = 'led_failed'
    elif not actual and b'PLLED1' not in rx:
        outcome = 'led_missing'
    elif len(actual) != rx.count(b'PLLED1') or actual != expected[:len(actual)]:
        outcome = 'led_unexpected_sequence'
    elif actual != expected:
        outcome = 'led_incomplete'
    else:
        outcome = 'pass'
    diagnostics = []
    for m in re.finditer(READBACK[min(version, 2)], rx):
        diagnostics.append({'pin': m[1].decode(), 'stage': m[2].decode(),
                            **{key: bool(int(m[index])) for index, key in enumerate(
                                ('want_high', 'latch_high', 'input_high'), 3)},
                            **({'input_high_samples': int(m[6]), 'input_samples': 64} if version > 1 else {}),
                            'directions_ok': bool(int(m[7])), 'stopped_ok': bool(int(m[8])),
                            'pad_control': m[9].decode()})
    verified = [p.decode() for p in LED_PINS if p not in output_only] if outcome == 'pass' else []
    return {'outcome': outcome, 'led_probe_verified': outcome == 'pass' and not output_only,
            'led_input_verified_pins': verified, 'led_readbacks': diagnostics,
            **({'led_output_only_pins': [p.decode() for p in output_only]} if output_only else {})}


def classify_flash(rx, arrival, version=1, end=None):
    """FW06 HELLO/READY binding, then one injected peer change and the 30 s watch.

    Version 2 (fw06-park-v1) checks only HELLO/READY: it refuses peer/done records
    and requires the park round trip's start record after READY instead.
    Version 3 (fw06-read-v1) checks HELLO/READY the same way, then the mediated
    flash read records (`classify_flash_read`); `end` is the capture end time.
    Version 4 (fw06-write-v1) does the same for the scratch-sector write (`classify_flash_write`).
    Version 5 (fw15-journal-v1) checks HELLO/READY as version 2, then requires the journal's
    start record after READY; the multi-boot journal records are left to its own auditor.
    Version 6 (fw15-settings-v1 and -v2) does the same with the settings store's start record and
    also refuses journal records; the power-cut settings records are left to its own auditor.
    `arrival(offset)` returns the host receive time of the rx chunk holding `offset`."""
    kinds = ('hello', 'ready', 'peer', 'done') if version == 1 else ('hello', 'ready')
    found = {kind: list(pattern.finditer(rx)) for kind, pattern in FLASH.items()}
    report = {'flash_handshake_verified': False, 'flash_authority_session': None, 'flash_boot_count': None,
              'flash_peer_session': None, 'flash_injected_peer_session': None,
              'flash_ready_seconds': None, 'flash_peer_change_seconds': None, 'flash_done_seconds': None}
    if version == 3:
        report.update(flash_read_verified=False, flash_read_id=None, flash_read_reads=None, flash_read_leases=None,
                      flash_read_park_max=None, flash_read_release_max=None, flash_read_frozen=None,
                      flash_read_alive=None, flash_read_seconds=None, flash_read_tail_seconds=None)
    if version == 4:
        report.update(flash_write_verified=False, flash_write_id=None, flash_write_preimage_blank=None, flash_write_cycles=[],
                      flash_write_restore_erase_ticks=None, flash_write_restore_blank=None, flash_write_leases=None,
                      flash_write_park_max=None, flash_write_release_max=None, flash_write_frozen=None,
                      flash_write_erase_ms_max=None, flash_write_seconds=None, flash_write_tail_seconds=None)
    single = {kind: found[kind][0] for kind in ('hello', 'ready', 'peer', 'done') if len(found[kind]) == 1}
    if 'hello' in single:
        report.update(flash_authority_session=single['hello'][1].decode(), flash_boot_count=int(single['hello'][2]))
    if 'ready' in single:
        report['flash_peer_session'] = single['ready'][1].decode()
    if 'peer' in single:
        report['flash_injected_peer_session'] = single['peer'][1].decode()
    if b'PLFLS1' not in rx:
        return {'outcome': 'flash_handshake_missing', **report}
    if rx.count(b'PLFLS1') != sum(len(v) for v in found.values()) or len(found['foreign']) > 1 or any(
            len(found[kind]) > 1 for kind in ('hello', 'ready', 'peer', 'done')):
        return {'outcome': 'flash_handshake_malformed', **report}
    if version >= 2 and (found['peer'] or found['done']):
        return {'outcome': 'flash_handshake_malformed', **report}
    # Under the read and write policies an error after READY belongs to that stage; earlier ones fail the handshake.
    if found['error'] and (version not in (3, 4) or 'ready' not in single or found['error'][0].start() < single['ready'].end()):
        return {'outcome': 'flash_handshake_failed', **report}
    if len(single) != len(kinds):
        return {'outcome': 'flash_handshake_missing', **report}
    led_done = rx.find(b'PLLED1 done low=all\r\n')
    if version == 3:
        return classify_flash_read(rx, arrival, end, single['hello'], single['ready'], found, led_done, report)
    if version == 4:
        return classify_flash_write(rx, arrival, end, single['hello'], single['ready'], found, led_done, report)
    if version == 5:
        if any(mark in rx for mark in (b'PLPRK1', b'PLRDS1', b'PLWRS1')):
            return {'outcome': 'flash_handshake_malformed', **report}
        return classify_flash_park(rx, arrival, single['hello'], single['ready'], found['foreign'], led_done, report, JOURNAL_MARK)
    if version == 6:
        if any(mark in rx for mark in (b'PLPRK1', b'PLRDS1', b'PLWRS1', b'PLJNL1')):
            return {'outcome': 'flash_handshake_malformed', **report}
        return classify_flash_park(rx, arrival, single['hello'], single['ready'], found['foreign'], led_done, report, SETTINGS_MARK)
    if version >= 2:
        return classify_flash_park(rx, arrival, single['hello'], single['ready'], found['foreign'], led_done, report)
    hello, ready, peer, done = (single[kind] for kind in kinds)
    if not (0 <= led_done < hello.start() < hello.end() <= ready.start() < ready.end() <= peer.start() < peer.end() <= done.start()) or any(
            m.start() < hello.end() for m in found['foreign']):
        return {'outcome': 'flash_handshake_order', **report}
    first, second = int(ready[1], 16), int(peer[1], 16)
    ok = (ready_fields_ok(hello, ready)
          and second != first and second >> 32 == (first >> 32) + 1
          and tuple(int(v) for v in peer.groups()[1:]) == FLASH_PEER
          and tuple(int(v) for v in done.groups()) == FLASH_DONE)
    if not ok:
        return {'outcome': 'flash_handshake_failed', **report}
    at = {kind: arrival(m.end() - 1) for kind, m in single.items()}
    report.update(flash_ready_seconds=(at['ready'] - at['hello']).total_seconds(),
                  flash_peer_change_seconds=(at['peer'] - at['ready']).total_seconds(),
                  flash_done_seconds=(at['done'] - at['ready']).total_seconds())
    if not (0 <= report['flash_ready_seconds'] <= 2.5 and 9.0 <= report['flash_peer_change_seconds'] <= 12.0
            and 29.0 <= report['flash_done_seconds'] <= 32.0):
        return {'outcome': 'flash_handshake_timing', **report}
    return {'outcome': 'pass', **report, 'flash_handshake_verified': True}


def ready_fields_ok(hello, ready):
    """HELLO authority carries the boot count; READY binds a distinct, nonzero peer session while stopped."""
    authority, count, peer = int(hello[1], 16), int(hello[2]), int(ready[1], 16)
    return (authority != 0 and authority >> 32 == count and count >= 1
            and peer != 0 and peer != authority and peer >> 32 >= 1
            and tuple(int(v) for v in ready.groups()[1:]) == FLASH_READY)


def classify_flash_park(rx, arrival, hello, ready, foreign, led_done, report, mark=PARK_MARK):
    """fw06-park-v1: HELLO/READY fields and timing, then the first park start record after READY.

    fw15-journal-v1 and fw15-settings-v1 apply the same checks with their start record as `mark`."""
    start = rx.find(mark)
    if start < 0:
        return {'outcome': 'flash_handshake_missing', **report}
    if not (0 <= led_done < hello.start() < hello.end() <= ready.start() < ready.end() <= start) or any(
            m.start() < hello.end() for m in foreign):
        return {'outcome': 'flash_handshake_order', **report}
    if not ready_fields_ok(hello, ready):
        return {'outcome': 'flash_handshake_failed', **report}
    report['flash_ready_seconds'] = (arrival(ready.end() - 1) - arrival(hello.end() - 1)).total_seconds()
    if not 0 <= report['flash_ready_seconds'] <= 2.5:
        return {'outcome': 'flash_handshake_timing', **report}
    return {'outcome': 'pass', **report, 'flash_handshake_verified': True}


def classify_flash_read(rx, arrival, end, hello, ready, flash, led_done, report):
    """fw06-read-v1: HELLO/READY as in park-v1, then one start, four batches and done of the mediated read."""
    found = {kind: list(pattern.finditer(rx)) for kind, pattern in READ.items()}
    start, done, batches = found['start'], found['done'], found['batch']
    if batches:
        last = batches[-1]
        ids = {m[2].decode() for m in batches}
        report.update(flash_read_id=ids.pop() if len(ids) == 1 else None,
                      flash_read_park_max=max(int(m[4]) for m in batches), flash_read_release_max=max(int(m[5]) for m in batches),
                      flash_read_frozen=int(last[6]), flash_read_alive=int(last[7]))
    if len(done) == 1:
        report.update(flash_read_reads=int(done[0][1]), flash_read_leases=int(done[0][2]))
    reads = [int(m[1]) for m in batches]
    if (rx.count(b'PLRDS1') != sum(len(v) for v in found.values()) or b'PLPRK1' in rx
            or any(len(found[kind]) > 1 for kind in ('start', 'done', 'skip')) or len(set(reads)) != len(reads)):
        return {'outcome': 'flash_read_malformed', **report}
    if found['skip'] or flash['error']:
        return {'outcome': 'flash_read_failed', **report}
    if not (0 <= led_done < hello.start() < hello.end() <= ready.start()) or any(m.start() < hello.end() for m in flash['foreign']):
        return {'outcome': 'flash_handshake_order', **report}
    if not ready_fields_ok(hello, ready):
        return {'outcome': 'flash_handshake_failed', **report}
    report['flash_ready_seconds'] = (arrival(ready.end() - 1) - arrival(hello.end() - 1)).total_seconds()
    if not 0 <= report['flash_ready_seconds'] <= 2.5:
        return {'outcome': 'flash_handshake_timing', **report}
    if not start or not done:
        return {'outcome': 'flash_read_missing', **report}
    start, done = start[0], done[0]
    # Batches lie between start and done in ascending order; no PLFLS1 record follows done.
    if not (ready.end() <= start.start() < start.end() <= done.start()) or reads != sorted(reads) or any(
            not start.end() <= m.start() < done.start() for m in batches) or any(m.start() > done.start() for m in flash['foreign']):
        return {'outcome': 'flash_read_order', **report}
    ok = (start.groups() == (b'00000000', b'pass', b'1') and tuple(reads) == READ_BATCHES
          and all(m[2].decode() == READ_ID and int(m[3]) == 0 and int(m[6]) == int(m[1]) and int(m[7]) >= int(m[1]) - 1
                  and 0 < int(m[4]) <= EXCHANGE_TICKS and 0 < int(m[5]) <= EXCHANGE_TICKS for m in batches)
          and tuple(int(v) for v in done.groups()) == READ_DONE)
    if not ok:
        return {'outcome': 'flash_read_failed', **report}
    finished = arrival(done.end() - 1)
    report.update(flash_read_seconds=(finished - arrival(start.end() - 1)).total_seconds(),
                  flash_read_tail_seconds=(end - finished).total_seconds())
    if not (0 <= report['flash_read_seconds'] <= 30.0 and report['flash_read_tail_seconds'] >= 30.0):
        return {'outcome': 'flash_read_timing', **report}
    return {'outcome': 'pass', **report, 'flash_handshake_verified': True, 'flash_read_verified': True}


def classify_flash_write(rx, arrival, end, hello, ready, flash, led_done, report):
    """fw06-write-v1: HELLO/READY as in park-v1, then start, preimage, three erase/program/verify cycles, restore and done."""
    found = {kind: list(pattern.finditer(rx)) for kind, pattern in WRITE.items()}
    cycles = found['cycle']
    single = {kind: found[kind][0] for kind in ('start', 'preimage', 'restore', 'done') if len(found[kind]) == 1}
    report['flash_write_cycles'] = [dict(zip(('n', 'erase_ticks', 'blank', 'program_max', 'errors'), (int(v) for v in m.groups())))
                                    for m in cycles]
    if 'preimage' in single:
        report.update(flash_write_id=single['preimage'][1].decode(), flash_write_preimage_blank=int(single['preimage'][2]))
    if 'restore' in single:
        report.update(flash_write_restore_erase_ticks=int(single['restore'][1]), flash_write_restore_blank=int(single['restore'][2]))
    if 'done' in single:
        done = [int(v) for v in single['done'].groups()]
        report.update(flash_write_leases=done[1], flash_write_park_max=done[4], flash_write_release_max=done[5], flash_write_frozen=done[6])
    erase = [c['erase_ticks'] for c in report['flash_write_cycles']] + (
        [report['flash_write_restore_erase_ticks']] if 'restore' in single else [])
    if erase:
        report['flash_write_erase_ms_max'] = round(max(erase) * 1000 / TICK_HZ, 1)
    numbers = [c['n'] for c in report['flash_write_cycles']]
    if (rx.count(b'PLWRS1') != sum(len(v) for v in found.values()) or b'PLPRK1' in rx or b'PLRDS1' in rx
            or any(len(found[kind]) > 1 for kind in ('start', 'preimage', 'restore', 'done', 'skip')) or len(set(numbers)) != len(numbers)):
        return {'outcome': 'flash_write_malformed', **report}
    if found['skip'] or flash['error']:
        return {'outcome': 'flash_write_failed', **report}
    if not (0 <= led_done < hello.start() < hello.end() <= ready.start()) or any(m.start() < hello.end() for m in flash['foreign']):
        return {'outcome': 'flash_handshake_order', **report}
    if not ready_fields_ok(hello, ready):
        return {'outcome': 'flash_handshake_failed', **report}
    report['flash_ready_seconds'] = (arrival(ready.end() - 1) - arrival(hello.end() - 1)).total_seconds()
    if not 0 <= report['flash_ready_seconds'] <= 2.5:
        return {'outcome': 'flash_handshake_timing', **report}
    if len(single) != 4:
        return {'outcome': 'flash_write_missing', **report}
    start, preimage, restore, done = (single[kind] for kind in ('start', 'preimage', 'restore', 'done'))
    # start, preimage, ascending cycles, restore, done; no PLFLS1 record follows done.
    if not (ready.end() <= start.start() < start.end() <= preimage.start() < preimage.end() <= restore.start() < restore.end() <= done.start()) or (
            numbers != sorted(numbers) or any(not preimage.end() <= m.start() < restore.start() for m in cycles)
            or any(m.start() > done.start() for m in flash['foreign'])):
        return {'outcome': 'flash_write_order', **report}
    done_fields = tuple(int(v) for v in done.groups())
    ok = (start.groups() == (b'00000000', b'pass', b'1')
          and preimage[1].decode() == READ_ID and int(preimage[2]) == WRITE_BLANK
          and tuple(numbers) == WRITE_CYCLES
          and all(c['blank'] == WRITE_BLANK and c['errors'] == 0 and 0 < c['erase_ticks'] <= ERASE_TICKS
                  and 0 < c['program_max'] <= PROGRAM_TICKS for c in report['flash_write_cycles'])
          and int(restore[2]) == WRITE_BLANK and 0 < int(restore[1]) <= ERASE_TICKS
          and done_fields[:4] == WRITE_DONE and done_fields[6] == WRITE_DONE[1]
          and 0 < done_fields[4] <= EXCHANGE_TICKS and 0 < done_fields[5] <= EXCHANGE_TICKS)
    if not ok:
        return {'outcome': 'flash_write_failed', **report}
    finished = arrival(done.end() - 1)
    report.update(flash_write_seconds=(finished - arrival(start.end() - 1)).total_seconds(),
                  flash_write_tail_seconds=(end - finished).total_seconds())
    if not (0 <= report['flash_write_seconds'] <= 60.0 and report['flash_write_tail_seconds'] >= 30.0):
        return {'outcome': 'flash_write_timing', **report}
    return {'outcome': 'pass', **report, 'flash_handshake_verified': True, 'flash_write_verified': True}


def classify(rx, identity, require_stopped=False):
    """Parse only fixed public fields; never return arbitrary device text."""
    starts = list(START.finditer(rx))
    outputs = list(OUTPUTS.finditer(rx))
    stopped = len(outputs) == 1 and tuple(int(v, 16) for v in outputs[0].groups()[:4]) == (0x6000, 0, 0xe000, 8) and outputs[0][5] == b'pass'
    outcome = 'timeout'
    if any(x in rx for x in (b'Hard Fault', b'HardFault', b'Fault on thread')):
        outcome = 'fault'
    elif rx.count(b'PLFW1 start ') > 1 or rx.count(b'Petlibre local firmware: Rust bringup') > 1:
        outcome = 'repeated_startup'
    elif outputs and not stopped or b'PLFW1 error outputs=' in rx:
        outcome = 'outputs_failed'
    elif b'PLFW1 error ' in rx or FAIL in rx or b'Synthetic core diagnostic: FAIL' in rx:
        outcome = 'diagnostic_failed'
    elif rx.count(b'PLFW1 ') != len(starts) + len(outputs) + rx.count(PASS) or rx.count(PASS) > 1:
        outcome = 'malformed_report'
    elif len(starts) == 1:
        m = starts[0]
        bank, reason, ctrl, start, offset = [int(x, 16) if x != b'unknown' else None for x in m.groups()[1:]]
        physical = start - offset if ctrl & 2 else start + offset
        live_bank = {0x08006000: 0, 0x08186000: 1}.get(physical) if ctrl & 1 and start == 0x0c000000 and 0 <= physical <= 0xffffffff else None
        if m[1].decode() != identity:
            outcome = 'wrong_build'
        elif bank is None or live_bank != bank:
            outcome = 'unknown_mapping'
        elif require_stopped and not stopped:
            outcome = 'outputs_missing'
        elif stopped and outputs[0].end() > m.start():
            outcome = 'outputs_after_startup'
        elif rx.count(PASS) == 1 and rx.index(PASS) >= m.end() and rx.count(b'Synthetic core diagnostic: PASS') == 1 and rx.count(b'Synthetic diagnostic only; feeder backend stays unopened.') == 1 and b'Flash Download Start' not in rx and b'app_main:version:' not in rx:
            outcome = 'pass'
        else:
            outcome = 'incomplete_report'
        return {'outcome': outcome, 'bank': bank, 'reset_flags': f'{reason:08x}', 'start_offset': m.start(), 'stopped_gpio_verified': stopped}
    return {'outcome': outcome}


def load(capture, manifest_path, image_path):
    """Bind the image to its source-bound manifest and validate a receive-only capture."""
    require = EVIDENCE.require
    manifest = json.loads(manifest_path.read_bytes())
    identity = digest(json.dumps({'profile': manifest['profile'], 'inputs': manifest['inputs']}, sort_keys=True, separators=(',', ':')).encode())
    require(manifest.get('startup_identity') == identity, 'Build identity is not source-bound')
    image = image_path.read_bytes()
    ota = [a for a in manifest['artifacts'] if Path(a['path']).name == 'OTA_All.bin']
    require(len(ota) == 1 and ota[0]['sha256'] == digest(image) and ota[0]['bytes'] == len(image), 'Artifact/manifest mismatch')
    require(f'PLFW1 start build={identity} '.encode() in image, 'Identity absent from artifact')
    metadata = json.loads((capture / 'report.json').read_bytes())
    rx, tx = [(capture / name).read_bytes() for name in ('serial-rx.bin', 'serial-tx.bin')]
    events = [json.loads(line) for line in (capture / 'serial-events.jsonl').read_text().splitlines()]
    EVIDENCE.analyze(rx, tx, events, metadata)
    require(not tx and metadata.get('mode') == 'observe', 'Receive-only observation required')
    return manifest, identity, image, metadata, rx, tx, events


def startup(rx, manifest, image, identity, arrival=None, end=None):
    """Classify one startup segment under every policy the manifest declares.

    `arrival(offset)` gives host receive times for timing-bound policies; `end`
    is the capture end time, needed for the flash read's and write's clean tail."""
    require = EVIDENCE.require
    supervision = manifest.get('startup_supervision_policy')
    require(supervision in (None, 'fw05-probe-v1'), 'Unknown startup supervision policy')
    require((supervision == 'fw05-probe-v1') == (SUPERVISION_MARK in image), 'Supervision policy/artifact mismatch')
    policy = manifest.get('startup_output_policy')
    require(policy in (None, 'stop-only'), 'Unknown startup output policy')
    require((policy == 'stop-only') == (b'PLFW1 outputs pa=' in image), 'Output policy/artifact mismatch')
    result = classify(rx, identity, require_stopped=policy == 'stop-only')
    clock_policy = manifest.get('startup_gpio_clock_policy')
    require(clock_policy in (None, 'explicit-enable-v1'), 'Unknown GPIO clock policy')
    require(not clock_policy or policy == 'stop-only', 'Clock profile requires stopped-output reporting')
    require((clock_policy == 'explicit-enable-v1') == (b'PLCLK1 function_before=' in image), 'GPIO clock policy/artifact mismatch')
    if clock_policy:
        clock_result = classify_clock(rx)
        if result['outcome'] == 'pass':
            if clock_result['outcome'] == 'pass' and CLOCK.search(rx).end() > OUTPUTS.search(rx).start():
                clock_result.update(outcome='gpio_clock_order', gpio_clock_verified=False)
            result.update(clock_result)
        else:
            result.update({k: v for k, v in clock_result.items() if k != 'outcome'})
    led_policy = manifest.get('startup_led_policy')
    require(led_policy in (None, 'bounded-probe-v1', 'bounded-probe-v2', 'bounded-probe-v3'), 'Unknown startup LED policy')
    require(bool(led_policy) == (b'PLLED1 begin' in image), 'LED policy/artifact mismatch')
    require((led_policy in ('bounded-probe-v2', 'bounded-probe-v3')) == (b' highs=' in image), 'LED readback format/artifact mismatch')
    require((led_policy == 'bounded-probe-v3') == (b'PLLED1 begin output_only=' in image), 'LED output-only policy/artifact mismatch')
    if led_policy:
        led_result = classify_led(rx, {'bounded-probe-v2': 2, 'bounded-probe-v3': 3}.get(led_policy, 1))
        if result['outcome'] == 'pass':
            if led_result['outcome'] == 'pass' and rx.index(b'PLLED1 begin') < rx.index(PASS) + len(PASS):
                led_result = {**led_result, 'outcome': 'led_unexpected_sequence', 'led_probe_verified': False, 'led_input_verified_pins': []}
            result.update(led_result)
        else:
            result.update(led_readbacks=led_result['led_readbacks'], led_probe_verified=False, led_input_verified_pins=[],
                          led_outcome=led_result['outcome'])
    routing_policy = manifest.get('startup_gpio_routing_policy')
    require(routing_policy in (None, 'read-only-v1', 'read-only-v2'), 'Unknown GPIO routing policy')
    require(bool(routing_policy) == (b'PLIO1 stage=' in image), 'GPIO routing policy/artifact mismatch')
    require((routing_policy == 'read-only-v2') == (b' share_pad=' in image), 'GPIO routing format/artifact mismatch')
    if routing_policy:
        routing = classify_routing(rx, 2 if routing_policy == 'read-only-v2' else 1)
        if result['outcome'] == 'pass':
            result.update(routing)
        else:
            result.update({k: v for k, v in routing.items() if k != 'outcome'})
    power_policy = manifest.get('startup_shared_pad_policy')
    require(power_policy in (None, 'before-km4-v1', 'before-km4-v2', 'before-km4-v3'), 'Unknown shared-pad policy')
    # Review 06 and earlier print PLPWR1 after KM4 boot without declaring this policy.
    require(not power_policy or b'PLPWR1 before_func=' in image, 'Shared-pad policy/artifact mismatch')
    # Checked both ways: a before-KM4 v3 image cannot be audited without its policy.
    require((power_policy == 'before-km4-v3') == (b'PLPWR1 before_func=' in image and POWER_V3_FORMAT in image),
            'Shared-pad v3 policy/artifact mismatch')
    if power_policy:
        power = classify_power(rx, {'before-km4-v2': 2, 'before-km4-v3': 3}.get(power_policy, 1))
        if result['outcome'] == 'pass':
            result.update(power)
        else:
            result.update({k: v for k, v in power.items() if k != 'outcome'})
    adc_policy = manifest.get('startup_adc_policy')
    require(adc_policy in (None, 'fw14-adc-v1', 'fw14-adc-v2'), 'Unknown ADC policy')
    require(bool(adc_policy) == (ADC_MARK in image), 'ADC policy/artifact mismatch')
    require((adc_policy == 'fw14-adc-v2') == (ADC_SUPPLY_MARK in image), 'ADC supply policy/artifact mismatch')
    require(not adc_policy or led_policy, 'ADC probe requires the LED probe policy')
    flash_policy = manifest.get('startup_flash_handshake_policy')
    require(flash_policy in (None, 'fw06-hello-v1', 'fw06-park-v1', 'fw06-read-v1', 'fw06-write-v1', 'fw15-journal-v1',
                            'fw15-settings-v1', 'fw15-settings-v2'), 'Unknown flash handshake policy')
    require(bool(flash_policy) == (FLASH_MARK in image), 'Flash handshake policy/artifact mismatch')
    # Checked both ways: a park image cannot be audited as a hello-only build, nor the reverse.
    require((flash_policy == 'fw06-park-v1') == (PARK_MARK in image), 'Flash park policy/artifact mismatch')
    require((flash_policy == 'fw06-read-v1') == (READ_MARK in image), 'Flash read policy/artifact mismatch')
    require((flash_policy == 'fw06-write-v1') == (WRITE_MARK in image), 'Flash write policy/artifact mismatch')
    require((flash_policy == 'fw15-journal-v1') == (JOURNAL_MARK in image), 'Flash journal policy/artifact mismatch')
    require((flash_policy in ('fw15-settings-v1', 'fw15-settings-v2')) == (SETTINGS_MARK in image),
            'Flash settings policy/artifact mismatch')
    if flash_policy:
        require(led_policy, 'Flash handshake requires the LED probe policy')
        require(arrival is not None, 'Flash handshake requires host receive times')
        require(flash_policy != 'fw06-read-v1' or end is not None, 'Flash read requires the capture end time')
        require(flash_policy != 'fw06-write-v1' or end is not None, 'Flash write requires the capture end time')
        flash = classify_flash(rx, arrival, {'fw06-park-v1': 2, 'fw06-read-v1': 3, 'fw06-write-v1': 4, 'fw15-journal-v1': 5,
                                           'fw15-settings-v1': 6, 'fw15-settings-v2': 6}.get(flash_policy, 1), end)
        if result['outcome'] == 'pass':
            result.update(flash)
        else:
            result.update({k: v for k, v in flash.items() if k != 'outcome'})
    return result


def verify(capture, manifest_path, image_path, minimum_seconds):
    manifest, identity, image, metadata, rx, tx, events = load(capture, manifest_path, image_path)
    chunks = [(e['offset'], e['offset'] + e['bytes'], e['utc']) for e in events if e['direction'] == 'rx']

    def arrival(offset):
        return EVIDENCE.timestamp(next(utc for start, end, utc in chunks if start <= offset < end))

    result = startup(rx, manifest, image, identity, arrival, EVIDENCE.timestamp(metadata['end_utc']))
    if result['outcome'] == 'pass':
        at = result.pop('start_offset')
        seconds = (EVIDENCE.timestamp(metadata['end_utc']) - arrival(at)).total_seconds()
        result['seconds_after_startup'] = seconds
        if seconds < minimum_seconds:
            result['outcome'] = 'observation_too_short'
    result.pop('start_offset', None)
    return {**result, 'build_id': identity, 'ota_sha256': digest(image), 'rx_sha256': digest(rx), 'tx_bytes': len(tx), 'minimum_seconds': minimum_seconds,
            'limit': 'UART startup evidence; electrical outputs and physical reset cause are not established.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('capture', 'manifest', 'image', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--minimum-seconds', type=float, default=60)
    args = parser.parse_args()
    if not 1 <= args.minimum_seconds <= 180:
        parser.error('Observation minimum must be 1–180 seconds')
    if not args.output.resolve().is_relative_to((ROOT / 'private').resolve()):
        parser.error('Evidence output must stay under private/')
    report = verify(args.capture, args.manifest, args.image, args.minimum_seconds)
    with args.output.open('x') as stream:
        stream.write(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return 0 if report['outcome'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
