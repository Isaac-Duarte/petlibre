"""Bounded host transport against actual helper instructions and fault peers."""
import importlib.util
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[3]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "src" / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


TOOL = module("recovery_transport", "flash/recovery-sector-transport.py")
SIM = module("recovery_peer", "emulate/emulate-recovery-session.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"


class Peer(SIM.EmulatedHelperPort):
    def __init__(self, helper, fault=None):
        self.fault, self.writes = fault, []
        self.program_ack_pending = self.eot_ack_pending = False
        super().__init__(helper, status_registers=(0, 2, 0x20))

    def write(self, data):
        self.writes.append(data)
        if len(data) == 1032 and data[0] == 2:
            self.program_ack_pending = True
        if data == b"\x04":
            self.eot_ack_pending = True
        return super().write(data)

    def read(self, count):
        data = super().read(count)
        if self.program_ack_pending and data == b"\x06":
            self.program_ack_pending = False
            if self.fault == "lost_ack":
                raise TimeoutError("Synthetic ACK loss after actual helper program")
            if self.fault == "nak":
                return b"\x15"
        if self.eot_ack_pending and data == b"\x06":
            self.eot_ack_pending = False
            if self.fault == "corrupt_readback":
                address = 0x08000000 + SIM.TARGET + 17
                value = bytes(self.uc.mem_read(address, 1))[0]
                self.uc.mem_write(address, bytes([value ^ 1]))
        return data

    def _step(self, uc, pc, size, data):
        if self.fault == "blocked_erase" and pc == 0x2146:
            self.calls.append({"name": "blocked_erase"})
            self._return()
            return
        super()._step(uc, pc, size, data)


@evidence(BASELINE.exists() and SIM.HELPER.exists(), "Requires private baseline and pinned helper")
class RecoveryTransportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline = BASELINE.read_bytes()
        cls.helper = SIM.HELPER.read_bytes()

    def peer(self, fault=None):
        port = Peer(self.helper, fault)
        before = TOOL.digest(port.before[SIM.TARGET:SIM.TARGET + 4096])
        return port, before

    def test_pinned_sector_verified_before_erase_and_after_program(self):
        port, before = self.peer()
        events = []
        result = TOOL.restore_sector(port, self.baseline, SIM.TARGET, before, events.append)
        payload = TOOL.sector_payload(self.baseline, SIM.TARGET)
        self.assertEqual(result["after_sha256"], TOOL.digest(payload))
        self.assertTrue(result["marker_withheld"])
        self.assertEqual(events[0]["event"], "before_verified")
        self.assertEqual(events[-1]["event"], "sector_verified")
        self.assertEqual(sum(c["name"] == "erase" for c in port.calls), 1)
        frames = [w for w in port.writes if len(w) == 1032]
        self.assertEqual([w[1] for w in frames], [1, 2, 3, 4])
        self.assertEqual(b"".join(w[7:-1] for w in frames), payload)
        after = port.snapshot()
        self.assertEqual(after[:SIM.TARGET], port.before[:SIM.TARGET])
        self.assertEqual(after[SIM.TARGET + 4096:], port.before[SIM.TARGET + 4096:])

    def test_invalid_provenance_or_sector_cannot_touch_transport(self):
        for baseline, offset, before in ((b"bad", SIM.TARGET, "a" * 64),
                                          (self.baseline, 0x6000, "a" * 64),
                                          (self.baseline, SIM.TARGET + 1, "a" * 64),
                                          (self.baseline, SIM.TARGET, "missing")):
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                TOOL.restore_sector(object(), baseline, offset, before, [].append)

    def test_idle_ready_bytes_arriving_during_durable_intent_are_drained_before_erase(self):
        port, before = self.peer()
        events = []

        def journal(event):
            events.append(event)
            if event["event"] == "erase_intent":
                # The actual helper emits a ready NAK every 50 ms while idle;
                # saving durable intent can take longer than that interval.
                port.outgoing.extend(b"\x15" * 4)

        result = TOOL.restore_sector(port, self.baseline, SIM.TARGET, before, journal)
        self.assertEqual(result["after_sha256"], TOOL.digest(TOOL.sector_payload(self.baseline, SIM.TARGET)))
        self.assertEqual(sum(c["name"] == "erase" for c in port.calls), 1)
        self.assertEqual(sum(len(w) == 6 and w[0] == 0x17 for w in port.writes), 1)
        self.assertFalse(any(e["event"] == "stopped" for e in events))

    def test_failed_journal_prevents_first_mutation(self):
        port, before = self.peer()
        events = []

        def journal(event):
            events.append(event)
            if event["event"] == "erase_intent":
                raise OSError("Synthetic durable-journal failure")

        with self.assertRaisesRegex(OSError, "journal"):
            TOOL.restore_sector(port, self.baseline, SIM.TARGET, before, journal)
        self.assertFalse(any(c["name"] in ("erase", "program") for c in port.calls))
        self.assertEqual(port.snapshot(), port.before)
        self.assertFalse(events[-1]["mutation_possible"])

    def test_before_hash_mismatch_cannot_erase(self):
        port, _before = self.peer()
        events = []
        with self.assertRaisesRegex(ValueError, "before hash"):
            TOOL.restore_sector(port, self.baseline, SIM.TARGET, "0" * 64, events.append)
        self.assertEqual(port.calls, [])
        self.assertFalse(events[-1]["mutation_possible"])

    def test_lost_ack_or_nak_is_terminal_uncertain_mutation_without_replay(self):
        for fault in ("lost_ack", "nak"):
            with self.subTest(fault=fault):
                port, before = self.peer(fault)
                events = []
                with self.assertRaises((TimeoutError, ValueError)):
                    TOOL.restore_sector(port, self.baseline, SIM.TARGET, before, events.append)
                self.assertEqual(sum(c["name"] == "program" for c in port.calls), 128)
                self.assertEqual(len([w for w in port.writes if len(w) == 1032]), 1)
                self.assertTrue(events[-1]["mutation_possible"])
                self.assertEqual(events[-1]["event"], "stopped")

    def test_ack_with_blocked_erase_cannot_reach_program(self):
        port, before = self.peer("blocked_erase")
        events = []
        with self.assertRaisesRegex(ValueError, "Erase readback"):
            TOOL.restore_sector(port, self.baseline, SIM.TARGET, before, events.append)
        self.assertFalse(any(len(w) == 1032 for w in port.writes))
        self.assertEqual(port.snapshot(), port.before)
        self.assertTrue(events[-1]["mutation_possible"])

    def test_program_ack_does_not_replace_exact_sector_readback(self):
        port, before = self.peer("corrupt_readback")
        events = []
        with self.assertRaisesRegex(ValueError, "Program readback"):
            TOOL.restore_sector(port, self.baseline, SIM.TARGET, before, events.append)
        self.assertTrue(events[-1]["mutation_possible"])
        self.assertFalse(any(e["event"] == "sector_verified" for e in events))

    def test_marker_commits_only_after_entire_staged_image_and_ranges_match(self):
        port, before = self.peer()
        # Other bank-1 sectors are already restored; retain every current outside byte.
        current = bytearray(port.before)
        port.uc.mem_write(0x08000000 + TOOL.START + 4096,
                          self.baseline[TOOL.START + 4096:TOOL.END])
        current[TOOL.START + 4096:TOOL.END] = self.baseline[TOOL.START + 4096:TOOL.END]
        events = []
        TOOL.restore_sector(port, self.baseline, TOOL.START, before, events.append)
        self.assertEqual(port.snapshot()[TOOL.START:TOOL.START + 8], b"\xff" * 8)
        erases = sum(c["name"] == "erase" for c in port.calls)
        result = TOOL.commit_marker(port, self.baseline, current, port.snapshot, events.append)
        self.assertEqual(sum(c["name"] == "erase" for c in port.calls), erases)
        self.assertEqual(result["program_packets"], 1)
        after = port.snapshot()
        self.assertEqual(after[TOOL.START:TOOL.END], self.baseline[TOOL.START:TOOL.END])
        self.assertEqual(after[:TOOL.START], current[:TOOL.START])
        self.assertEqual(after[TOOL.END:], current[TOOL.END:])
        self.assertEqual(events[-1]["event"], "marker_commit_verified")

    def test_incomplete_image_or_changed_settings_blocks_marker_transmission(self):
        for changed_offset in (TOOL.START + 4096, 0x3E8000):
            with self.subTest(changed_offset=hex(changed_offset)):
                port, before = self.peer()
                current = port.before
                port.uc.mem_write(0x08000000 + TOOL.START, self.baseline[TOOL.START:TOOL.END])
                port.uc.mem_write(0x08000000 + TOOL.START, b"\xff" * 8)
                address = 0x08000000 + changed_offset
                value = bytes(port.uc.mem_read(address, 1))[0]
                port.uc.mem_write(address, bytes([value ^ 1]))
                events = []
                with self.assertRaisesRegex(ValueError, "Full staged image"):
                    TOOL.commit_marker(port, self.baseline, current, port.snapshot, events.append)
                self.assertEqual(port.calls, [])
                self.assertEqual(port.writes, [])
                self.assertFalse(events[-1]["mutation_possible"])

    def test_already_staged_marker_sector_is_verified_without_erase(self):
        port, _before = self.peer()
        payload = TOOL.sector_payload(self.baseline, TOOL.START)
        port.uc.mem_write(0x08000000 + TOOL.START, payload)
        events = []
        result = TOOL.restore_sector(port, self.baseline, TOOL.START, TOOL.digest(payload), events.append)
        self.assertEqual(result["program_packets"], 0)
        self.assertEqual(port.calls, [])
        self.assertEqual(events[-1]["event"], "marker_already_staged")

    def test_lost_commit_ack_is_uncertain_even_when_marker_was_programmed(self):
        port, before = self.peer()
        current = port.before
        port.uc.mem_write(0x08000000 + TOOL.START + 4096,
                          self.baseline[TOOL.START + 4096:TOOL.END])
        events = []
        TOOL.restore_sector(port, self.baseline, TOOL.START, before, events.append)
        port.fault = "lost_ack"
        writes_before = len(port.writes)
        with self.assertRaises(TimeoutError):
            TOOL.commit_marker(port, self.baseline, current, port.snapshot, events.append)
        # The exact body was complete before the marker call; lost ACK cannot
        # justify retrying a commit that may already have made stock bootable.
        self.assertEqual(port.snapshot()[TOOL.START:TOOL.END], self.baseline[TOOL.START:TOOL.END])
        self.assertEqual(len([w for w in port.writes[writes_before:] if len(w) == 1032]), 1)
        self.assertEqual(events[-1]["event"], "stopped")
        self.assertTrue(events[-1]["mutation_possible"])


if __name__ == "__main__":
    unittest.main()
