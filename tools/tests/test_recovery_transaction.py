"""Multi-sector recovery, true marker-last commit and durable audit failures."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from support import evidence
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


TOOL = module("recovery_transaction", "flash/recovery-transaction.py")
SIM = module("recovery_transaction_peer", "emulate/emulate-recovery-session.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"


class ActivationPeer(SIM.EmulatedHelperPort):
    def __init__(self, helper, current, baseline, fault=None, before_write=None):
        self.baseline, self.current = baseline, current
        self.activation_calls = self.program_packets = 0
        self.fault, self.pending_program_ack = fault, False
        self.before_write = before_write
        super().__init__(helper, current, [TOOL.PLAN.START, TOOL.PLAN.START + 4096], status_registers=(0, 2, 0x20))

    def _step(self, uc, pc, size, data):
        if pc == 0x2048:
            offset = self.uc.reg_read(SIM.arm.UC_ARM_REG_R0)
            pointer = self.uc.reg_read(SIM.arm.UC_ARM_REG_R2)
            if offset == TOOL.PLAN.START and bytes(self.uc.mem_read(pointer, 8)) != b"\xff" * 8:
                # Observe at the actual first marker program call, before NOR changes.
                staged = self.snapshot()
                assert staged[TOOL.PLAN.START + 8:TOOL.PLAN.END] == self.baseline[TOOL.PLAN.START + 8:TOOL.PLAN.END]
                assert staged[:TOOL.PLAN.START] == self.current[:TOOL.PLAN.START]
                assert staged[TOOL.PLAN.END:] == self.current[TOOL.PLAN.END:]
                self.activation_calls += 1
        super()._step(uc, pc, size, data)

    def write(self, data):
        if self.before_write and ((len(data) == 6 and data[0] == 0x17) or len(data) == 1032):
            self.before_write(data)
        if len(data) == 1032:
            self.program_packets += 1
            self.pending_program_ack = True
        return super().write(data)

    def read(self, count):
        data = super().read(count)
        if self.pending_program_ack and data == b"\x06":
            self.pending_program_ack = False
            if self.fault == "lost_program_ack":
                raise TimeoutError("Synthetic lost program ACK")
        return data


@evidence(BASELINE.exists() and SIM.HELPER.exists(), "Requires private baseline and vendor helper")
class RecoveryTransactionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline = BASELINE.read_bytes()
        cls.helper = SIM.HELPER.read_bytes()

    def setUp(self):
        (ROOT / "private/verification").mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT / "private/verification", prefix="transaction-test-")
        self.parent = Path(self.temporary.name)
        self.addCleanup(self.temporary.cleanup)
        current = bytearray(self.baseline)
        current[TOOL.PLAN.START] = 0  # Invalidated old stock marker.
        current[TOOL.PLAN.START + 4096 + 17] ^= 1
        current[0x3E8000] ^= 1       # Newer settings and an unclassified area stay current.
        current[0x600000] ^= 1
        self.current = bytes(current)
        self.manifest = TOOL.PLAN.plan(self.baseline, self.current)

    def peer(self, current=None, **kwargs):
        return ActivationPeer(self.helper, current if current is not None else self.current,
                              self.baseline, **kwargs)

    def run_plan(self, peer, output=None, snapshot=None, manifest=None, current=None):
        return TOOL.run_transaction(peer, self.baseline, current if current is not None else self.current,
                                    manifest if manifest is not None else self.manifest,
                                    snapshot if snapshot is not None else peer.snapshot,
                                    output if output is not None else self.parent / "run")

    def test_actual_helper_transaction_has_synced_intents_and_true_marker_last(self):
        output = self.parent / "run"
        synced_heads = []
        real_sync = TOOL.os.fsync

        def sync(fd):
            real_sync(fd)
            path = output / "journal.jsonl"
            if path.exists() and os.fstat(fd).st_ino == path.stat().st_ino:
                synced_heads.append(TOOL.inspect_journal(path)["head_sha256"])

        def before_write(data):
            inspected = TOOL.inspect_journal(output / "journal.jsonl")
            self.assertEqual(inspected["head_sha256"], synced_heads[-1])
            last = json.loads((output / "journal.jsonl").read_text().splitlines()[-1])["event"]
            self.assertEqual(last["event"], "erase_intent" if len(data) == 6 else "program_packet_intent")

        peer = self.peer(before_write=before_write)
        with patch.object(TOOL.os, "fsync", side_effect=sync):
            report = self.run_plan(peer, output)
        inspected = TOOL.inspect_journal(output / "journal.jsonl")
        self.assertTrue(inspected["verified_terminal"])
        self.assertEqual(inspected["erase_order"], [TOOL.PLAN.START + 4096, TOOL.PLAN.START])
        self.assertEqual(peer.activation_calls, 1)
        self.assertEqual(peer.program_packets, 9)  # Four + four staged packets, one no-erase commit.
        self.assertEqual(sum(c["name"] == "erase" for c in peer.calls), 2)
        after = peer.snapshot()
        self.assertEqual(after[TOOL.PLAN.START:TOOL.PLAN.END], self.baseline[TOOL.PLAN.START:TOOL.PLAN.END])
        self.assertEqual(after[:TOOL.PLAN.START], self.current[:TOOL.PLAN.START])
        self.assertEqual(after[TOOL.PLAN.END:], self.current[TOOL.PLAN.END:])
        self.assertEqual(report["snapshot_sha256"], self.manifest["proposed_snapshot_sha256"])
        self.assertEqual((output / "journal.jsonl").stat().st_mode & 0o777, 0o600)
        self.assertEqual((output / "report.json").stat().st_mode & 0o777, 0o600)
        with self.assertRaises(FileExistsError):
            self.run_plan(peer, output)

    def test_altered_plan_order_hash_or_bounds_cannot_touch_peer(self):
        variants = []
        for field, value in (("target_bank", True), ("current_snapshot_sha256", "0" * 64)):
            changed = copy.deepcopy(self.manifest)
            changed[field] = value
            variants.append(changed)
        changed = copy.deepcopy(self.manifest)
        changed["ordered_sector_payloads"].reverse()
        variants.append(changed)
        changed = copy.deepcopy(self.manifest)
        changed["ordered_sector_payloads"][0]["offset"] = 0x6000
        variants.append(changed)
        for manifest in variants:
            with self.assertRaisesRegex(ValueError, "Plan differs"):
                self.run_plan(object(), snapshot=lambda: self.fail("Snapshot requested"), manifest=manifest)
        self.assertFalse((self.parent / "run").exists())

    def test_changed_fresh_snapshot_aborts_before_serial_mutation(self):
        peer = self.peer()
        with self.assertRaisesRegex(ValueError, "Fresh full snapshot"):
            self.run_plan(peer, snapshot=lambda: self.baseline)
        self.assertEqual(peer.calls, [])
        journal = TOOL.inspect_journal(self.parent / "run/journal.jsonl")
        self.assertEqual(journal["terminal_event"], "transaction_stopped")

    def test_settings_change_detected_before_any_marker_commit(self):
        peer = self.peer()
        snapshots = 0

        def snapshot():
            nonlocal snapshots
            snapshots += 1
            if snapshots == 2:
                address = 0x08000000 + 0x3E8000
                value = bytes(peer.uc.mem_read(address, 1))[0]
                peer.uc.mem_write(address, bytes([value ^ 1]))
            return peer.snapshot()

        with self.assertRaisesRegex(ValueError, "Full staged image"):
            self.run_plan(peer, snapshot=snapshot)
        self.assertEqual(peer.activation_calls, 0)
        self.assertEqual(peer.snapshot()[TOOL.PLAN.START:TOOL.PLAN.START + 8], b"\xff" * 8)
        self.assertFalse(TOOL.inspect_journal(self.parent / "run/journal.jsonl")["verified_terminal"])

    def test_lost_ack_stops_without_activating_or_replaying(self):
        peer = self.peer(fault="lost_program_ack")
        with self.assertRaises(TimeoutError):
            self.run_plan(peer)
        self.assertEqual(peer.program_packets, 1)
        self.assertEqual(peer.activation_calls, 0)
        self.assertEqual(peer.snapshot()[TOOL.PLAN.START], self.current[TOOL.PLAN.START])
        report = json.loads((self.parent / "run/report.json").read_text())
        self.assertEqual(report["status"], "stopped")
        self.assertTrue(report["mutation_possible"])

    def test_intent_journal_failure_prevents_first_erase(self):
        original = TOOL.Journal.append

        def append(journal, event):
            if event["event"] == "erase_intent":
                raise OSError("Synthetic intent persistence failure")
            return original(journal, event)

        peer = self.peer()
        with patch.object(TOOL.Journal, "append", append), self.assertRaises(OSError):
            self.run_plan(peer)
        self.assertFalse(any(c["name"] in ("erase", "program") for c in peer.calls))
        self.assertEqual(peer.snapshot(), self.current)

    def test_failed_fsync_latches_journal_and_never_appends_again(self):
        journal = TOOL.Journal(self.parent / "failed-journal")
        self.addCleanup(journal.close)
        with patch.object(TOOL.os, "fsync", side_effect=OSError("Synthetic disk failure")):
            with self.assertRaises(OSError):
                journal.append({"event": "erase_intent", "offset": TOOL.PLAN.START})
        self.assertTrue(journal.failed)
        with self.assertRaisesRegex(ValueError, "failed previously"):
            journal.append({"event": "transaction_verified"})

    def test_journal_corruption_partial_line_and_complete_prefix_are_not_success(self):
        journal = TOOL.Journal(self.parent / "journal-check")
        journal.append({"event": "transaction_start"})
        journal.append({"event": "transaction_verified"})
        journal.close()
        path = self.parent / "journal-check/journal.jsonl"
        original = path.read_bytes()
        partial = self.parent / "partial.jsonl"
        partial.write_bytes(original[:-1])
        with self.assertRaisesRegex(ValueError, "incomplete"):
            TOOL.inspect_journal(partial)
        corrupt = self.parent / "corrupt.jsonl"
        corrupt.write_bytes(original.replace(b"transaction_verified", b"transaction_modified"))
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            TOOL.inspect_journal(corrupt)
        prefix = self.parent / "prefix.jsonl"
        prefix.write_bytes(original.splitlines(keepends=True)[0])
        self.assertFalse(TOOL.inspect_journal(prefix)["verified_terminal"])

    def test_unchanged_snapshot_has_no_sector_or_marker_operations(self):
        snapshots = []

        def snapshot():
            snapshots.append(1)
            return self.baseline

        manifest = TOOL.PLAN.plan(self.baseline, self.baseline)
        report = self.run_plan(object(), snapshot=snapshot, manifest=manifest, current=self.baseline)
        self.assertEqual(report["sectors"], [])
        self.assertIsNone(report["marker_commit"])
        self.assertEqual(len(snapshots), 1)

    def test_new_reviewed_transaction_can_commit_already_staged_image_without_erase(self):
        current = bytearray(self.current)
        current[TOOL.PLAN.START:TOOL.PLAN.END] = self.baseline[TOOL.PLAN.START:TOOL.PLAN.END]
        current[TOOL.PLAN.START:TOOL.PLAN.START + 8] = b"\xff" * 8
        current = bytes(current)
        peer = self.peer(current)
        report = self.run_plan(peer, current=current, manifest=TOOL.PLAN.plan(self.baseline, current))
        self.assertEqual(sum(c["name"] == "erase" for c in peer.calls), 0)
        self.assertEqual(peer.activation_calls, 1)
        self.assertEqual(report["sectors"][0]["program_packets"], 0)

    def test_old_plan_cannot_replay_under_a_new_output_after_success(self):
        peer = self.peer()
        self.run_plan(peer)
        calls = len(peer.calls)
        with self.assertRaisesRegex(ValueError, "Fresh full snapshot"):
            self.run_plan(peer, output=self.parent / "replay")
        self.assertEqual(len(peer.calls), calls)

    def test_protected_busy_or_suspended_status_blocks_first_mutation(self):
        for index, values in enumerate(((0x1C, 2, 0x20), (1, 2, 0x20), (0, 0x82, 0x20), (0, 6, 0x20))):
            with self.subTest(values=values):
                peer = self.peer()
                peer.status_registers = dict(zip((5, 53, 21), values))
                output = self.parent / f"blocked-{index}"
                with self.assertRaisesRegex(ValueError, "Flash status blocks before_erase"):
                    self.run_plan(peer, output=output)
                self.assertFalse(any(c["name"] in ("erase", "program") for c in peer.calls))
                self.assertEqual(peer.snapshot(), self.current)
                self.assertEqual(peer.activation_calls, 0)
                self.assertFalse(json.loads((output / "report.json").read_text())["mutation_possible"])

    def test_new_busy_state_after_erase_stops_before_programming(self):
        class Peer(ActivationPeer):
            def _step(self, uc, pc, size, data):
                super()._step(uc, pc, size, data)
                if pc == 0x2146:
                    self.status_registers[5] = 1

        peer = Peer(self.helper, self.current, self.baseline)
        with self.assertRaisesRegex(ValueError, "Flash status blocks before_program"):
            self.run_plan(peer)
        self.assertEqual(sum(c["name"] == "erase" for c in peer.calls), 1)
        self.assertEqual(peer.program_packets, 0)
        self.assertEqual(peer.activation_calls, 0)
        self.assertTrue(json.loads((self.parent / "run/report.json").read_text())["mutation_possible"])

    def test_new_protection_after_staging_prevents_marker_commit(self):
        class Peer(ActivationPeer):
            def _step(self, uc, pc, size, data):
                if pc == 0x1D1C and sum(c["name"] == "program" for c in self.calls) >= 1024:
                    self.status_registers[5] = 0x1C
                super()._step(uc, pc, size, data)

        peer = Peer(self.helper, self.current, self.baseline)
        with self.assertRaisesRegex(ValueError, "Flash status blocks before_marker_commit"):
            self.run_plan(peer)
        self.assertEqual(peer.program_packets, 8)
        self.assertEqual(peer.activation_calls, 0)
        self.assertEqual(peer.snapshot()[TOOL.PLAN.START:TOOL.PLAN.START + 8], b"\xff" * 8)
        self.assertEqual(sum(c["name"] == "erase" for c in peer.calls), 2)

    def test_changed_status_between_read_triples_stops_without_erase(self):
        class Peer(ActivationPeer):
            def _step(self, uc, pc, size, data):
                if pc == 0x1D1C and self.uc.reg_read(SIM.arm.UC_ARM_REG_R0) == 5:
                    if any(c.get("opcode") == 5 for c in self.calls):
                        self.status_registers[53] = 0  # Both configurations are individually idle/unprotected.
                super()._step(uc, pc, size, data)

        peer = Peer(self.helper, self.current, self.baseline)
        with self.assertRaisesRegex(ValueError, "changed between observations"):
            self.run_plan(peer)
        self.assertFalse(any(c["name"] in ("erase", "program") for c in peer.calls))
        self.assertEqual(peer.snapshot(), self.current)

    def test_missing_status_reply_stops_before_first_erase(self):
        class Peer(ActivationPeer):
            def read(self, count):
                data = super().read(count)
                if any(c["name"] == "read_status" for c in self.calls) and data == b"\x21":
                    raise TimeoutError("Synthetic missing status echo")
                return data

        peer = Peer(self.helper, self.current, self.baseline)
        with self.assertRaises(TimeoutError):
            self.run_plan(peer)
        self.assertFalse(any(c["name"] in ("erase", "program") for c in peer.calls))
        self.assertEqual(peer.snapshot(), self.current)
