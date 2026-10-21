"""Pinned diagnostic round trip, wire policy and terminal failures with real helper instructions."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from support import evidence
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


TOOL = module("standalone_cli_test", "uart-standalone.py")
BANK = TOOL.BANK
SIM = module("standalone_helper_test", "emulate-recovery-session.py")
WIRE = module("standalone_wire_test", "standalone-wire-policy.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"
CANDIDATE = ROOT / "private/fixtures/diagnostic-ota.bin"
CURRENT = ROOT / "private/fixtures/current-read/flash-read.bin"
SMALL = ROOT / "private/fixtures/bounded-read/report.json"
ROM = ROOT / "private/fixtures/rom-inspection/report.json"


@evidence(BASELINE.is_file() and CANDIDATE.is_file() and SIM.HELPER.is_file(),
                     "Requires pinned private baseline, UART-only candidate and helper")
class StandaloneBankTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline, cls.candidate, cls.helper = BASELINE.read_bytes(), CANDIDATE.read_bytes(), SIM.HELPER.read_bytes()
        source = bytearray(cls.baseline)
        for offset in (0x3e8000, 0x650000):
            source[offset] ^= 1
        cls.stock = bytes(source)
        cls.custom = BANK.images(cls.baseline, cls.stock, cls.candidate, "install")[0]

    def temporary(self):
        temporary = tempfile.TemporaryDirectory(dir=ROOT / "private")
        self.addCleanup(temporary.cleanup)
        return Path(temporary.name)

    def reviewed(self, current, direction):
        return {"schema_version": 1, "kind": BANK.KINDS[direction], "port": "/fixture",
                "operation_plan": BANK.plan(self.baseline, current, self.candidate, direction),
                "tool_sha256": {"fixture": "a" * 64}, "current_report_sha256": "b" * 64}

    def run_operation(self, direction, fault=None, serial=False):
        current = self.stock if direction == "install" else self.custom
        proposed, staged = BANK.images(self.baseline, current, self.candidate, direction)
        planned = BANK.plan(self.baseline, current, self.candidate, direction)
        state = {"snapshots": [], "activation_observations": 0, "body_erases_while_withheld": 0}

        class Peer(SIM.EmulatedHelperPort):
            pending_lost_ack = False

            def _step(self, uc, pc, size, data):
                if pc == 0x2146 and fault == "blocked_erase":
                    self._return()
                    return
                if pc in (0x2146, 0x2048):
                    offset = uc.reg_read(SIM.arm.UC_ARM_REG_R1 if pc == 0x2146 else SIM.arm.UC_ARM_REG_R0)
                    if offset >= BANK.START + BANK.SIZE:
                        assert bytes(uc.mem_read(0x08000000 + BANK.START, 8)) == b"\xff" * 8
                        if pc == 0x2146:
                            state["body_erases_while_withheld"] += 1
                    if pc == 0x2048 and offset == BANK.START:
                        pointer = uc.reg_read(SIM.arm.UC_ARM_REG_R2)
                        if bytes(uc.mem_read(pointer, 8)) != b"\xff" * 8:
                            assert self.snapshot() == staged
                            state["activation_observations"] += 1
                super()._step(uc, pc, size, data)

            def write(self, data):
                if data == BANK.R.read_request(BANK.START + BANK.SIZE, BANK.SIZE):
                    if fault == "body_drift":
                        at = 0x08000000 + BANK.START + BANK.SIZE + 17
                        self.uc.mem_write(at, bytes([self.uc.mem_read(at, 1)[0] ^ 1]))
                    if fault == "body_protection":
                        self.status_registers[0x05] = 0x1c
                if data == BANK.R.read_request(BANK.START, BANK.SIZE) and len(state["snapshots"]) == 2:
                    if fault == "marker_recheck_drift":
                        at = 0x08000000 + BANK.START + 17
                        self.uc.mem_write(at, bytes([self.uc.mem_read(at, 1)[0] ^ 1]))
                    if fault == "commit_protection":
                        self.status_registers[0x05] = 0x1c
                if len(data) == 6 and data[0] == 0x17 and fault == "lost_erase_ack":
                    self.pending_lost_ack = True
                if len(data) == 1032 and fault == "lost_program_ack":
                    self.pending_lost_ack = True
                if len(data) == 1032 and data[7:15] != b"\xff" * 8 and \
                        int.from_bytes(data[3:7], "little") == 0x08000000 + BANK.START and fault == "lost_commit_ack":
                    self.pending_lost_ack = True
                result = super().write(data)
                if data == b"\x04" and fault == "program_readback":
                    at = 0x08000000 + BANK.START + 17
                    self.uc.mem_write(at, bytes([self.uc.mem_read(at, 1)[0] ^ 1]))
                return result

            def read(self, count):
                data = super().read(count)
                if self.pending_lost_ack and data == b"\x06":
                    self.pending_lost_ack = False
                    raise TimeoutError("Modeled acknowledgment loss after mutation")
                return data

        peer = Peer(self.helper, current, [s["offset"] for s in planned["ordered_sector_payloads"]],
                    status_registers=(0x1c, 2, 0) if fault == "protected" else (0, 2, 0))
        output = self.temporary() / "result"

        def snapshot():
            data = peer.snapshot()
            number = len(state["snapshots"]) + 1
            if (fault == "initial_drift" and number == 1) or \
                    (fault == "staged_drift" and number == 2) or (fault == "final_drift" and number == 3):
                damaged = bytearray(data)
                damaged[0x650000] ^= 1
                data = bytes(damaged)
            state["snapshots"].append(data)
            return data

        original_append = BANK.C.Journal.append

        def append(journal, event):
            if (fault == "erase_journal" and event["event"] == "erase_intent") or \
                    (fault == "commit_journal" and event["event"] == "marker_commit_intent"):
                raise OSError("Modeled durable intent failure")
            result = original_append(journal, event)
            if fault == "idle_during_intent" and event["event"] == "erase_intent":
                peer.outgoing.extend(b"\x15" * 4)
            return result

        with patch.object(BANK.C.Journal, "append", append):
            if serial:
                peer.open = peer.close = lambda: None

                def full_read(_port, offset, length, stream, _progress):
                    self.assertEqual((offset, length), (0, 0x800000))
                    data = snapshot()
                    stream.write(data[:1024] if fault == "truncated_snapshot" else data)
                    return {"bytes": len(data), "packets": 8192, "duplicate_packets": 0}

                reviewed = self.reviewed(current, direction)
                args = SimpleNamespace(output=output, port="/fixture", mode=direction)
                with patch.dict(sys.modules, {"serial": SimpleNamespace(Serial=lambda **_: peer)}), \
                        patch.object(TOOL, "receive_snapshot", side_effect=full_read), \
                        patch.object(TOOL.R, "upload_helper", side_effect=AssertionError("Unexpected helper upload")), \
                        patch("sys.stdout", new_callable=io.StringIO):
                    state["code"] = TOOL.execute(args, self.baseline, current, self.candidate,
                                                  reviewed, TOOL.canonical(reviewed))
                state["session"] = json.loads((output / "session.json").read_text())
                state["journal_root"] = output / "transaction"
            else:
                try:
                    state["report"] = BANK.run(peer, self.baseline, current, self.candidate, direction,
                                                planned, snapshot, output)
                    state["code"] = 0
                except (ValueError, TimeoutError, OSError) as error:
                    state.update(code=1, error=str(error))
                state["journal_root"] = output
        state.update(peer=peer, output=output, current=current, proposed=proposed, staged=staged)
        return state

    def test_both_directions_match_independent_wire_and_marker_order(self):
        for direction in ("install", "restore"):
            with self.subTest(direction=direction):
                result = self.run_operation(direction)
                self.assertEqual(result["code"], 0, result.get("error"))
                self.assertEqual(bytes(result["peer"].host_tx), WIRE.expected(
                    self.baseline, self.candidate, direction, include_full_reads=False))
                self.assertEqual(result["snapshots"], [result["current"], result["staged"], result["proposed"]])
                self.assertEqual(result["peer"].snapshot(), result["proposed"])
                self.assertEqual(result["activation_observations"], 1)
                self.assertEqual(result["body_erases_while_withheld"], 25)
                receipt = BANK.C.inspect_journal(result["journal_root"] / "journal.jsonl")
                self.assertTrue(receipt["verified_terminal"])
                self.assertEqual(receipt["erase_order"], list(range(BANK.START, BANK.START + 106496, 4096)))

    def test_durable_intent_idle_bytes_do_not_change_wire_or_replay_erases(self):
        for direction in ("install", "restore"):
            with self.subTest(direction=direction):
                result = self.run_operation(direction, fault="idle_during_intent")
                self.assertEqual(result["code"], 0, result.get("error"))
                self.assertEqual(bytes(result["peer"].host_tx), WIRE.expected(
                    self.baseline, self.candidate, direction, include_full_reads=False))
                self.assertEqual(sum(c["name"] == "erase" for c in result["peer"].calls), 26)
                self.assertEqual(result["peer"].snapshot(), result["proposed"])

    def test_cli_retains_three_snapshots_and_serial_hashes_for_both_modes(self):
        for direction in ("install", "restore"):
            with self.subTest(direction=direction):
                result = self.run_operation(direction, serial=True)
                self.assertEqual(result["code"], 0, result["session"].get("error"))
                self.assertEqual(result["session"]["status"], "verified")
                self.assertEqual(len(result["session"]["snapshots"]), 3)
                for name in ("serial-rx.bin", "serial-tx.bin"):
                    self.assertEqual(BANK.digest((result["output"] / name).read_bytes()), result["session"][name + "_sha256"])
                self.assertFalse(result["session"]["helper_upload_attempted"])

    def test_identity_factory_other_bank_and_operation_guards(self):
        fixtures = [(self.baseline, self.stock, self.candidate + b"x", "install"),
                    (self.baseline, self.custom, self.candidate, "install"),
                    (self.baseline, self.stock, self.candidate, "restore"),
                    (self.baseline, self.stock, self.candidate, "other")]
        for offset in (0x100, BANK.START + 4096, BANK.END - 1, 0x6000):
            damaged = bytearray(self.stock)
            if offset == 0x6000:
                damaged[offset:offset + 8] = self.baseline[BANK.START:BANK.START + 8]
            else:
                damaged[offset] ^= 1
            fixtures.append((self.baseline, bytes(damaged), self.candidate, "install"))
        mutated_candidate = bytearray(self.candidate)
        mutated_candidate[100] ^= 1
        fixtures.append((self.baseline, self.stock, bytes(mutated_candidate), "install"))
        for baseline, current, candidate, direction in fixtures:
            with self.subTest(direction=direction, current=BANK.digest(current)):
                with self.assertRaises(ValueError):
                    BANK.plan(baseline, current, candidate, direction)
        with self.assertRaisesRegex(ValueError, "marker is still valid"):
            BANK.C.PLAN.plan(self.baseline, self.custom)

    def test_changed_plan_order_range_payload_type_and_kind_fail_before_io(self):
        original = BANK.plan(self.baseline, self.stock, self.candidate, "install")
        changed = []
        for field, value in (("sector_erases", 25), ("commit_packets", True),
                             ("kind", BANK.KINDS["restore"]), ("candidate_sha256", "0" * 64)):
            changed.append({**original, field: value})
        for modification in ("reverse", "range", "payload"):
            bad = copy.deepcopy(original)
            if modification == "reverse":
                bad["ordered_sector_payloads"].reverse()
            elif modification == "range":
                bad["target_range"]["end_exclusive"] += 4096
            else:
                bad["ordered_sector_payloads"][0]["stage_sha256"] = "0" * 64
            changed.append(bad)
        for bad in changed:
            output = self.temporary() / "uncreated"
            with self.assertRaisesRegex(ValueError, "plan differs"):
                BANK.run(object(), self.baseline, self.stock, self.candidate, "install", bad,
                         lambda: self.fail("Snapshot reached"), output)
            self.assertFalse(output.exists())

    def test_failures_before_erase_never_mutate(self):
        for fault in ("initial_drift", "protected", "erase_journal"):
            with self.subTest(fault=fault):
                result = self.run_operation("install", fault)
                self.assertEqual(result["code"], 1)
                self.assertEqual(result["peer"].snapshot(), self.stock)
                self.assertFalse(any(c["name"] in ("erase", "program") for c in result["peer"].calls))

    def test_mid_write_failures_stop_without_activation_or_replay_in_both_directions(self):
        faults = ("body_drift", "body_protection", "lost_erase_ack", "lost_program_ack", "blocked_erase",
                  "program_readback", "staged_drift", "marker_recheck_drift", "commit_protection", "commit_journal")
        for direction in ("install", "restore"):
            for fault in faults:
                with self.subTest(direction=direction, fault=fault):
                    result = self.run_operation(direction, fault)
                    self.assertEqual(result["code"], 1)
                    self.assertEqual(result["activation_observations"], 0)
                    receipt = BANK.C.inspect_journal(result["journal_root"] / "journal.jsonl")
                    self.assertFalse(receipt["verified_terminal"])
                    erases = [c for c in result["peer"].calls if c["name"] == "erase"]
                    if fault in ("body_drift", "body_protection", "lost_erase_ack", "lost_program_ack", "program_readback"):
                        self.assertEqual(len(erases), 1)
                    if fault == "lost_program_ack":
                        self.assertEqual(sum(c["name"] == "program" for c in result["peer"].calls), 128)

    def test_commit_ack_loss_and_final_drift_are_terminal_even_after_activation(self):
        for direction in ("install", "restore"):
            for fault in ("lost_commit_ack", "final_drift"):
                with self.subTest(direction=direction, fault=fault):
                    result = self.run_operation(direction, fault, serial=True)
                    self.assertEqual(result["code"], 1)
                    self.assertEqual(result["session"]["status"], "stopped")
                    self.assertEqual(result["activation_observations"], 1)
                    self.assertFalse(BANK.C.inspect_journal(result["journal_root"] / "journal.jsonl")["verified_terminal"])

    def test_truncated_full_snapshot_retains_partial_without_mutation(self):
        result = self.run_operation("install", "truncated_snapshot", serial=True)
        self.assertEqual(result["code"], 1)
        self.assertTrue((result["output"] / "snapshot-01.bin.part").exists())
        self.assertFalse(result["peer"].calls)

    def test_plan_hash_sources_port_types_and_explicit_cli_direction(self):
        expected = self.reviewed(self.stock, "install")
        for key, value in (("schema_version", True), ("port", "/other"), ("tool_sha256", {})):
            bad = {**expected, key: value}
            raw = TOOL.canonical(bad)
            with self.assertRaises(ValueError):
                TOOL.validate_reviewed(raw, TOOL.digest(raw), expected)
        raw = TOOL.canonical(expected)
        with self.assertRaisesRegex(ValueError, "SHA256"):
            TOOL.validate_reviewed(raw, "0" * 64, expected)
        wrong = self.reviewed(self.custom, "restore")
        argv = ["uart-standalone.py", "install", "--baseline", "unused", "--current", "unused",
                "--candidate", "unused", "--rom-report", "unused", "--small-report", "unused",
                "--output", "unused", "--port", "/fixture", "--plan", "unused",
                "--expected-plan-sha256", TOOL.digest(TOOL.canonical(wrong))]
        with patch.object(sys, "argv", argv), patch.object(TOOL.os, "umask"), \
                patch.object(TOOL, "prepare_inputs", return_value=(self.baseline, self.custom, self.candidate, wrong)), \
                patch.object(TOOL, "private_file", return_value=TOOL.canonical(wrong)), \
                patch.object(TOOL, "execute", side_effect=AssertionError("Execution reached")):
            with self.assertRaisesRegex(ValueError, "execution mode"):
                TOOL.main()

    def test_existing_output_is_terminal_before_serial_open(self):
        output = self.temporary()
        args = SimpleNamespace(output=output, port="/fixture", mode="install")
        reviewed = self.reviewed(self.stock, "install")
        with patch.dict(sys.modules, {"serial": SimpleNamespace(Serial=lambda **_: self.fail("Serial opened"))}):
            with self.assertRaises(FileExistsError):
                TOOL.execute(args, self.baseline, self.stock, self.candidate, reviewed, TOOL.canonical(reviewed))

    @evidence(CURRENT.is_file() and SMALL.is_file() and ROM.is_file(), "Requires closed-session provenance fixtures")
    def test_preparation_requires_complete_raw_full_read_and_same_helper_evidence(self):
        args = SimpleNamespace(mode="prepare-install", baseline=BASELINE, current=CURRENT, candidate=CANDIDATE,
                               rom_report=ROM, small_report=SMALL, port="/dev/ttyUSB0")
        baseline, current, candidate, reviewed = TOOL.prepare_inputs(args)
        self.assertEqual(reviewed["operation_plan"], BANK.plan(baseline, current, candidate, "install"))
        self.assertEqual(set(reviewed["tool_sha256"]), set(TOOL.SOURCES))
        temporary = self.temporary()
        for name in ("report.json", "serial-rx.bin", "serial-tx.bin"):
            (temporary / name).write_bytes((CURRENT.parent / name).read_bytes())
        (temporary / "flash-read.bin").write_bytes(current)
        args.current = temporary / "flash-read.bin"
        original = json.loads((temporary / "report.json").read_text())
        for field, value in (("status", "running"), ("helper_upload_attempted", True),
                             ("bounded_read_report_sha256", "0" * 64), ("port", "/other"),
                             ("serial-tx.bin_sha256", "0" * 64)):
            (temporary / "report.json").write_text(json.dumps({**original, field: value}))
            with self.subTest(field=field), self.assertRaises(ValueError):
                TOOL.prepare_inputs(args)


if __name__ == "__main__":
    unittest.main()
