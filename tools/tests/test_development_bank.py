"""Region updater against actual helper instructions, timing and failure peers."""
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from support import evidence as requires_evidence
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


B = module("development_bank_test", "development-bank.py")
SIM = module("development_peer_test", "emulate-recovery-session.py")
W = module("development_wire_test", "development-wire-policy.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"
SEED = ROOT / "private/fixtures/diagnostic-ota.bin"


def container(payload):
    return struct.pack("<8I", 0xffffffff, 1, 0x3141544f, 24,
                       sum(payload) & 0xffffffff, len(payload), 32, 0xffffffff) + payload


def variant(image, grow=False):
    """Synthetic code bytes for transport tests only; never a boot candidate."""
    payload = bytearray(B.payload(image))
    split = B.L.regions(payload)[2]["payload_header_offset"]
    payload[split + 4096 + 53] ^= 1
    payload[split + 8192 + 71] ^= 1
    if grow:
        size = struct.unpack_from("<I", payload, split + 8)[0]
        end = split + 32 + size
        payload[end:end] = b"\xa7" * 4096
        struct.pack_into("<I", payload, split + 8, size + 4096)
    return container(bytes(payload))


@requires_evidence(BASELINE.is_file() and SEED.is_file() and SIM.HELPER.is_file(),
                     "Requires original backup, diagnostic seed and pinned helper")
class DevelopmentBankTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline, cls.seed, cls.helper = BASELINE.read_bytes(), SEED.read_bytes(), SIM.HELPER.read_bytes()

    def temporary(self):
        directory = tempfile.TemporaryDirectory(dir=ROOT / "private")
        self.addCleanup(directory.cleanup)
        return Path(directory.name)

    def operation(self, source=None, target=None, fault=None, wire_reads=False, plan_change=None):
        source = source or self.seed
        target = target or variant(source)
        full = bytearray(self.baseline)
        full[B.START:B.START + len(B.payload(source))] = B.payload(source)
        full[0x3e8000] ^= 1
        full[0x650000] ^= 1
        full = bytes(full)
        current = full[B.START:B.START + B.extent(source, target)]
        planned = B.plan(source, target, current)
        if plan_change:
            plan_change(planned)
        proposed, staged = B.images(source, target, current)
        state = {"acquisitions": 0, "activation_calls": 0}

        class Peer(SIM.EmulatedHelperPort):
            pending_lost_ack = False

            def _step(self, uc, pc, size, data):
                if pc == 0x2146:
                    at = uc.reg_read(SIM.arm.UC_ARM_REG_R1)
                    if at != B.START:
                        assert bytes(uc.mem_read(0x08000000 + B.START, 8)) == b"\xff" * 8
                    if fault == "blocked_erase":
                        self._return()
                        return
                if pc == 0x2048 and uc.reg_read(SIM.arm.UC_ARM_REG_R0) == B.START:
                    pointer = uc.reg_read(SIM.arm.UC_ARM_REG_R2)
                    if bytes(uc.mem_read(pointer, 8)) != b"\xff" * 8:
                        assert self.snapshot()[B.START:B.START + len(staged)] == staged
                        state["activation_calls"] += 1
                super()._step(uc, pc, size, data)

            def write(self, data):
                if fault == "body_drift" and len(planned["ordered_sectors"]) > 1 and \
                        data == B.R.read_request(planned["ordered_sectors"][1]["offset"], B.SECTOR):
                    at = 0x08000000 + planned["ordered_sectors"][1]["offset"] + 71
                    self.uc.mem_write(at, bytes([self.uc.mem_read(at, 1)[0] ^ 1]))
                if len(data) == 6 and data[0] == 0x17 and fault == "lost_erase_ack":
                    self.pending_lost_ack = True
                if len(data) == 6 and data[0] == 0x17 and fault == "lost_second_erase_ack" and \
                        int.from_bytes(data[1:4], "little") == planned["ordered_sectors"][1]["offset"]:
                    self.pending_lost_ack = True
                if len(data) == 1032 and fault == "lost_program_ack":
                    self.pending_lost_ack = True
                if len(data) == 1032 and data[7:15] != b"\xff" * 8 and \
                        int.from_bytes(data[3:7], "little") == 0x08000000 + B.START and fault == "lost_commit_ack":
                    self.pending_lost_ack = True
                if data == B.R.read_request(B.START, B.SECTOR) and state["acquisitions"] == 2:
                    if fault == "marker_drift":
                        at = 0x08000000 + B.START + 71
                        self.uc.mem_write(at, bytes([self.uc.mem_read(at, 1)[0] ^ 1]))
                    if fault == "commit_protection":
                        self.status_registers[5] = 0x1c
                result = super().write(data)
                if data == b"\x04" and fault == "bad_readback":
                    at = 0x08000000 + B.START + 71
                    self.uc.mem_write(at, bytes([self.uc.mem_read(at, 1)[0] ^ 1]))
                return result

            def read(self, count):
                data = super().read(count)
                if self.pending_lost_ack and data == b"\x06":
                    self.pending_lost_ack = False
                    raise TimeoutError("Modeled missing ACK after mutation")
                return data

        offsets = [e["offset"] for e in planned["ordered_sectors"]] or [B.START]
        peer = Peer(self.helper, full, offsets, status_registers=(0x1c, 2, 0) if fault == "protected" else (0, 2, 0))

        def acquire():
            state["acquisitions"] += 1
            if wire_reads:
                import io
                values = []
                for at, size in (*B.GUARDS, (B.START, len(current))):
                    stream = io.BytesIO()
                    B.R.receive_flash(peer, at, size, stream)
                    values.append(stream.getvalue())
                data, protected = values[2], values[:2]
            else:
                snapshot = peer.snapshot()
                data = snapshot[B.START:B.START + len(current)]
                protected = [snapshot[o:o + n] for o, n in B.GUARDS]
            if (fault == "initial_drift" and state["acquisitions"] == 1) or \
                    (fault == "staged_drift" and state["acquisitions"] == 2) or \
                    (fault == "final_drift" and state["acquisitions"] == 3):
                data = data[:-1] + bytes([data[-1] ^ 1])
            if fault == "guard_drift" or (fault == "final_guard_drift" and state["acquisitions"] == 3):
                protected[0] = b"\x00" + protected[0][1:]
            return data, protected

        original = B.C.Journal.append

        def append(journal, event):
            if (fault == "erase_journal" and event["event"] == "erase_intent") or \
                    (fault == "commit_journal" and event["event"] == "marker_commit_intent"):
                raise OSError("Modeled durable intent failure")
            value = original(journal, event)
            if fault == "idle_during_intent" and event["event"] == "erase_intent":
                peer.outgoing.extend(b"\x15" * 4)
            return value

        output = self.temporary() / "transaction"
        with patch.object(B.C.Journal, "append", append):
            try:
                state["report"] = B.run(peer, self.baseline, source, target, current, planned, acquire, output)
                state["success"] = True
            except (ValueError, TimeoutError, OSError) as error:
                state.update(success=False, error=str(error))
        state.update(peer=peer, output=output, current=current, before=full, source=source,
                     target=target, proposed=proposed, staged=staged, planned=planned)
        return state

    def test_sparse_update_matches_independent_full_wire_and_preserves_unscanned_flash(self):
        r = self.operation(wire_reads=True)
        self.assertTrue(r["success"], r.get("error"))
        self.assertEqual(r["planned"]["erase_sectors"], 3)
        self.assertEqual(bytes(r["peer"].host_tx), W.expected(r["source"], r["target"], r["current"]))
        after = r["peer"].snapshot()
        self.assertEqual(after[:B.START], r["before"][:B.START])
        self.assertEqual(after[B.START + len(r["current"]):], r["before"][B.START + len(r["current"]):])
        self.assertEqual(after[B.START:B.START + len(r["current"])], r["proposed"])
        self.assertEqual(r["activation_calls"], 1)

    def test_growing_and_shrinking_images_verify_union_and_retain_tail(self):
        grown = variant(self.seed, grow=True)
        for source, target in [(self.seed, grown), (grown, self.seed)]:
            with self.subTest(growing=len(target) > len(source)):
                r = self.operation(source, target)
                self.assertTrue(r["success"], r.get("error"))
                self.assertEqual(len(r["current"]), max(len(source), len(target)) - 32)
                self.assertEqual(bytes(r["peer"].host_tx), W.expected(source, target, r["current"], include_acquisitions=False))
                self.assertEqual(r["proposed"][len(B.payload(target)):], r["current"][len(B.payload(target)):])

    def test_identical_image_is_read_only(self):
        r = self.operation(target=self.seed)
        self.assertTrue(r["success"], r.get("error"))
        self.assertEqual(r["acquisitions"], 1)
        self.assertEqual(r["peer"].calls, [])
        self.assertEqual(r["activation_calls"], 0)

    def test_stock_transition_and_original_return_use_only_required_sectors(self):
        stock = (ROOT / "private/firmware/PLAF103_OTA_010115.bin").read_bytes()
        for source, target in [(stock, self.seed), (self.seed, stock)]:
            with self.subTest(to_stock=target == stock):
                r = self.operation(source, target)
                self.assertTrue(r["success"], r.get("error"))
                self.assertEqual(r["planned"]["erase_sectors"], 26)
                self.assertEqual(bytes(r["peer"].host_tx), W.expected(source, target, r["current"], include_acquisitions=False))
                after = r["peer"].snapshot()
                self.assertEqual(after[:B.START], r["before"][:B.START])
                self.assertEqual(after[B.END:], r["before"][B.END:])

    def test_timing_regression_keeps_exact_commands_without_replay(self):
        r = self.operation(fault="idle_during_intent")
        self.assertTrue(r["success"], r.get("error"))
        self.assertEqual(bytes(r["peer"].host_tx), W.expected(r["source"], r["target"], r["current"], include_acquisitions=False))
        self.assertEqual(sum(c["name"] == "erase" for c in r["peer"].calls), 3)

    def test_preconditions_never_erase(self):
        for fault in ("initial_drift", "guard_drift", "protected", "erase_journal"):
            with self.subTest(fault=fault):
                r = self.operation(fault=fault)
                self.assertFalse(r["success"])
                self.assertFalse(any(c["name"] in ("erase", "program") for c in r["peer"].calls))
                self.assertEqual(r["activation_calls"], 0)

    def test_failures_stop_before_activation_without_replay(self):
        for fault in ("blocked_erase", "lost_erase_ack", "lost_program_ack", "bad_readback", "body_drift",
                      "staged_drift", "marker_drift", "commit_protection", "commit_journal"):
            with self.subTest(fault=fault):
                r = self.operation(fault=fault)
                self.assertFalse(r["success"])
                self.assertEqual(r["activation_calls"], 0)
                receipt = B.C.inspect_journal(r["output"] / "journal.jsonl")
                self.assertEqual(receipt["terminal_event"], "transaction_stopped")
                self.assertFalse(receipt["verified_terminal"])
                if fault == "lost_erase_ack":
                    self.assertEqual(sum(c["name"] == "erase" for c in r["peer"].calls), 1)
                    self.assertFalse(any(c["name"] == "program" for c in r["peer"].calls))

    def test_commit_uncertainty_never_claims_success_or_replays(self):
        for fault in ("lost_commit_ack", "final_drift", "final_guard_drift"):
            with self.subTest(fault=fault):
                r = self.operation(fault=fault)
                self.assertFalse(r["success"])
                self.assertEqual(r["activation_calls"], 1)
                self.assertFalse(B.C.inspect_journal(r["output"] / "journal.jsonl")["verified_terminal"])

    def interrupted(self, snapshot, source, target):
        """Finish from a fresh region acquisition of a stopped update's flash."""
        current = snapshot[B.START:B.START + B.extent(source, target)]
        planned = B.plan(source, target, current, interrupted=True)
        offsets = [B.START] + [e["offset"] for e in planned["ordered_sectors"]]  # The commit programs START.
        peer = SIM.EmulatedHelperPort(self.helper, snapshot, offsets, status_registers=(0, 2, 0))

        def acquire():
            data = peer.snapshot()
            return data[B.START:B.START + len(current)], [data[o:o + n] for o, n in B.GUARDS]
        report = B.run(peer, self.baseline, source, target, current, planned, acquire,
                       self.temporary() / "transaction", interrupted=True)
        return report, peer, planned

    def test_interrupted_update_finishes_from_a_fresh_acquisition(self):
        stopped = self.operation(fault="lost_second_erase_ack")
        self.assertFalse(stopped["success"])
        source, target = stopped["source"], stopped["target"]
        snapshot = stopped["peer"].snapshot()
        second = stopped["planned"]["ordered_sectors"][1]["offset"]
        self.assertEqual(snapshot[second:second + B.SECTOR], b"\xff" * B.SECTOR)
        with self.assertRaisesRegex(ValueError, "source image"):
            B.plan(source, target, snapshot[B.START:B.START + B.extent(source, target)])
        report, peer, planned = self.interrupted(snapshot, source, target)
        self.assertEqual(report["status"], "verified_via_supplied_transports")
        self.assertTrue(planned["interrupted"])
        self.assertEqual([e["offset"] for e in planned["ordered_sectors"]],
                         [e["offset"] for e in stopped["planned"]["ordered_sectors"][1:]])
        after = peer.snapshot()
        self.assertEqual(after[B.START:B.START + len(stopped["proposed"])], stopped["proposed"])
        self.assertEqual(after[:B.START], snapshot[:B.START])
        self.assertEqual(after[B.END:], snapshot[B.END:])
        # The old sector (not yet erased) and a fully staged region commit too.
        old = bytearray(snapshot)
        old[second:second + B.SECTOR] = stopped["before"][second:second + B.SECTOR]
        self.assertEqual(self.interrupted(bytes(old), source, target)[1].snapshot()[B.START:B.START + len(stopped["proposed"])],
                         stopped["proposed"])
        staged = bytearray(snapshot)
        staged[B.START:B.START + len(stopped["staged"])] = stopped["staged"]
        report, peer, planned = self.interrupted(bytes(staged), source, target)
        self.assertEqual(planned["ordered_sectors"], [])
        self.assertEqual(peer.snapshot()[B.START:B.START + len(stopped["proposed"])], stopped["proposed"])

    def test_interrupted_update_refuses_anything_but_source_staged_or_erased(self):
        stopped = self.operation(fault="lost_second_erase_ack")
        source, target, snapshot = stopped["source"], stopped["target"], stopped["peer"].snapshot()
        second = stopped["planned"]["ordered_sectors"][1]["offset"]
        size = B.extent(source, target)
        partial = bytearray(snapshot)
        partial[second:second + 1024] = stopped["staged"][second - B.START:second - B.START + 1024]
        committed = bytearray(snapshot)
        committed[B.START:B.START + len(stopped["proposed"])] = stopped["proposed"]
        unstaged = bytearray(snapshot)
        unstaged[B.START:B.START + B.SECTOR] = b"\xff" * B.SECTOR
        for name, image, message in (("partial program", partial, "neither source"),
                                     ("already committed", committed, "staged marker"),
                                     ("marker sector erased", unstaged, "staged marker"),
                                     ("original source", stopped["before"], "staged marker")):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, message):
                B.plan(source, target, bytes(image)[B.START:B.START + size], interrupted=True)

    def test_altered_plan_is_refused_before_transport(self):
        for field, value in [("erase_sectors", True), ("bank", 0), ("current_region_sha256", "0" * 64)]:
            with self.subTest(field=field):
                r = self.operation(plan_change=lambda p: p.update({field: value}))
                self.assertFalse(r["success"])
                self.assertEqual(bytes(r["peer"].host_tx), b"")

    def test_bad_images_or_unknown_source_are_refused(self):
        for image in (b"bad", self.seed[:-1], self.seed[:16] + b"\x00" * 4 + self.seed[20:], bytearray(self.seed)):
            with self.subTest(length=len(image)), self.assertRaises(ValueError):
                B.payload(image)
        current = b"\x00" * len(B.payload(self.seed))
        with self.assertRaisesRegex(ValueError, "source image"):
            B.plan(self.seed, variant(self.seed), current)


if __name__ == "__main__":
    unittest.main()
