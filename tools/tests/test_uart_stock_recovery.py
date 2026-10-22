"""Full serial orchestration with actual helper instructions and modeled snapshots."""
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


TOOL = module("stock_serial_recovery", "flash/uart-stock-recovery.py")
SIM = module("stock_serial_peer", "emulate/emulate-recovery-session.py")
WIRE = module("stock_bank_wire", "flash/stock-bank-wire-policy.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"


@evidence(BASELINE.exists() and SIM.HELPER.exists(), "Requires pinned private evidence")
class StockSerialRecoveryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline, cls.helper = BASELINE.read_bytes(), SIM.HELPER.read_bytes()
        current = bytearray(cls.baseline)
        current[TOOL.PLAN.START] = 0
        current[TOOL.PLAN.START + 4096 + 19] ^= 1
        current[0x3E8000] ^= 1
        cls.current = bytes(current)

    def reviewed(self, current=None, marker_test=False, bank_test=False):
        kind = TOOL.MARKER.BANK_KIND if bank_test else (TOOL.MARKER.MARKER_KIND if marker_test else TOOL.RECOVERY_KIND)
        return {"schema_version": 1, "kind": kind, "port": "/fixture",
                "recovery_plan": TOOL.operation_plan(self.baseline, current or self.current, kind),
                "tool_sha256": {"fixture": "a" * 64}, "current_report_sha256": "b" * 64}

    def test_plan_source_identity_and_canonical_types_are_checked_before_execution(self):
        expected = self.reviewed()
        for field, value in (("schema_version", True), ("port", "/other"),
                             ("tool_sha256", {"fixture": "c" * 64})):
            reviewed = {**expected, field: value}
            raw = TOOL.canonical(reviewed)
            with self.subTest(field=field), self.assertRaises(ValueError):
                TOOL.validate_reviewed(raw, TOOL.digest(raw), self.baseline, self.current, expected)
        reviewed = copy.deepcopy(expected)
        reviewed["recovery_plan"]["ordered_sector_payloads"].reverse()
        raw = TOOL.canonical(reviewed)
        with self.assertRaises(ValueError):
            TOOL.validate_reviewed(raw, TOOL.digest(raw), self.baseline, self.current, expected)
        raw = TOOL.canonical(expected)
        with self.assertRaisesRegex(ValueError, "SHA256 differs"):
            TOOL.validate_reviewed(raw, "0" * 64, self.baseline, self.current, expected)
        self.assertEqual(TOOL.validate_reviewed(raw, TOOL.digest(raw), self.baseline, self.current, expected), expected)

    def test_invalid_plan_cli_cannot_open_serial_or_create_output(self):
        expected = self.reviewed()
        argv = ["flash/uart-stock-recovery.py", "execute", "--baseline", "unused", "--current", "unused",
                "--rom-report", "unused", "--small-report", "unused", "--output", "unused",
                "--port", "/fixture", "--plan", "unused", "--expected-plan-sha256", "0" * 64]
        with patch.object(sys, "argv", argv), patch.object(TOOL.os, "umask"), \
                patch.object(TOOL, "prepare_inputs", return_value=(self.baseline, self.current, expected)), \
                patch.object(TOOL, "private_file", return_value=TOOL.canonical(expected)), \
                patch.object(TOOL, "execute", side_effect=AssertionError("Execution before plan validation")):
            with self.assertRaises(ValueError):
                TOOL.main()

    def run_serial(self, current=None, fault=None, marker_test=False, bank_test=False):
        current = current or self.current
        reviewed = self.reviewed(current, marker_test, bank_test)
        baseline, start = self.baseline, TOOL.PLAN.START
        state = {"snapshots": [], "activation_observations": [], "body_erases_while_withheld": 0}

        class Peer(SIM.EmulatedHelperPort):
            pending_program_ack = False
            pending_body_erase_ack = False

            def _step(self, uc, pc, size, data):
                if pc in (0x2146, 0x2048):
                    offset = uc.reg_read(SIM.arm.UC_ARM_REG_R1 if pc == 0x2146 else SIM.arm.UC_ARM_REG_R0)
                    if bank_test and offset >= start + 4096:
                        assert bytes(uc.mem_read(0x08000000 + start, 8)) == b"\xff" * 8
                        if pc == 0x2146:
                            state["body_erases_while_withheld"] += 1
                if pc == 0x2048:
                    offset = uc.reg_read(SIM.arm.UC_ARM_REG_R0)
                    pointer = uc.reg_read(SIM.arm.UC_ARM_REG_R2)
                    if offset == start and bytes(uc.mem_read(pointer, 8)) != b"\xff" * 8:
                        staged = self.snapshot()
                        assert staged[start:start + 8] == b"\xff" * 8
                        assert staged[start + 8:TOOL.PLAN.END] == baseline[start + 8:TOOL.PLAN.END]
                        assert staged[:start] == current[:start] and staged[TOOL.PLAN.END:] == current[TOOL.PLAN.END:]
                        state["activation_observations"].append(True)
                super()._step(uc, pc, size, data)

            def write(self, data):
                if data == TOOL.MARKER.READER.read_request(start + 4096, 4096):
                    if fault == "body_preimage_drift":
                        address = 0x08000000 + start + 4096
                        self.uc.mem_write(address, bytes([self.uc.mem_read(address, 1)[0] ^ 1]))
                    if fault == "body_protected":
                        self.status_registers[0x05] = 0x1C
                if len(data) == 6 and data[0] == 0x17 and int.from_bytes(data[1:4], "little") > start:
                    self.pending_body_erase_ack = fault == "lost_body_erase_ack"
                if len(data) == 1032 and data[0] == 2:
                    if fault == "lost_marker_ack":
                        self.pending_program_ack = bool(state["activation_observations"]) or (
                            int.from_bytes(data[3:7], "little") == 0x08000000 + start and
                            data[7:15] != b"\xff" * 8)
                    elif fault == "lost_first_ack":
                        self.pending_program_ack = True
                return super().write(data)

            def read(self, count):
                data = super().read(count)
                if self.pending_body_erase_ack and data == b"\x06":
                    self.pending_body_erase_ack = False
                    raise TimeoutError("Supplied lost body erase ACK after mutation")
                if self.pending_program_ack and data == b"\x06":
                    self.pending_program_ack = False
                    raise TimeoutError("Supplied lost ACK after programming")
                return data

        peer = Peer(self.helper, current, range(start, TOOL.PLAN.END, 4096) if bank_test else [start, start + 4096],
                    status_registers=(0x1C, 2, 0) if fault == "protected" else (0, 2, 0))
        peer.open = peer.close = lambda: None
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "private").mkdir()
        output = root / "private/result"
        args = SimpleNamespace(output=output, port="/fixture")

        def snapshot(_port, offset, length, stream, _progress):
            self.assertEqual((offset, length), (0, 0x800000))
            data = peer.snapshot()
            state["snapshots"].append(data)
            if fault == "initial_drift" and len(state["snapshots"]) == 1:
                data = bytearray(data)
                data[0x600000] ^= 1
                data = bytes(data)
            if fault == "staged_settings_drift" and len(state["snapshots"]) == 2:
                data = bytearray(data)
                data[0x3E8000] ^= 1
                data = bytes(data)
            if fault == "final_drift" and len(state["snapshots"]) == 3:
                data = bytearray(data)
                data[0x600000] ^= 1
                data = bytes(data)
            if fault == "truncated_snapshot":
                data = data[:1024]
            stream.write(data)
            return {"bytes": length, "packets": length // 1024, "duplicate_packets": 0}

        with patch.object(TOOL.READER, "ROOT", root), patch.object(TOOL.COORDINATOR, "ROOT", root), \
                patch.object(TOOL.MARKER.COORDINATOR, "ROOT", root), \
                patch.dict(sys.modules, {"serial": SimpleNamespace(Serial=lambda **_: peer)}), \
                patch.object(TOOL.READER, "receive_flash", side_effect=snapshot), \
                patch.object(TOOL.READER, "upload_helper", side_effect=AssertionError("Helper overwritten")), \
                patch("sys.stdout", new_callable=io.StringIO):
            code = TOOL.execute(args, self.baseline, current, reviewed, TOOL.canonical(reviewed))
        state.update(code=code, peer=peer, output=output,
                     session=json.loads((output / "session.json").read_text()),
                     report=json.loads((output / "transaction/report.json").read_text()))
        return state

    def test_serial_restore_stages_before_activation_and_uses_three_full_snapshots(self):
        result = self.run_serial()
        self.assertEqual(result["code"], 0, result["session"].get("error"))
        self.assertEqual(result["session"]["status"], "verified")
        self.assertEqual(len(result["snapshots"]), 3)
        self.assertEqual(len(result["activation_observations"]), 1)
        peer = result["peer"]
        self.assertEqual(sum(c["name"] == "erase" for c in peer.calls), 2)
        expected = self.current[:TOOL.PLAN.START] + self.baseline[TOOL.PLAN.START:TOOL.PLAN.END] + self.current[TOOL.PLAN.END:]
        self.assertEqual(peer.snapshot(), expected)
        self.assertFalse(result["session"]["helper_upload_attempted"])
        for name in ("serial-rx.bin", "serial-tx.bin"):
            self.assertEqual(TOOL.digest((result["output"] / name).read_bytes()), result["session"][name + "_sha256"])
        receipt = TOOL.COORDINATOR.inspect_journal(result["output"] / "transaction/journal.jsonl")
        self.assertTrue(receipt["verified_terminal"])
        self.assertEqual(receipt["erase_order"], [TOOL.PLAN.START + 4096, TOOL.PLAN.START])

    def test_noop_plan_opens_no_mutation_path_and_uses_one_snapshot(self):
        result = self.run_serial(current=self.baseline)
        self.assertEqual(result["code"], 0)
        self.assertEqual(len(result["snapshots"]), 1)
        self.assertEqual(result["peer"].calls, [])
        self.assertFalse(result["report"]["marker_commit"])

    def test_drift_or_lost_ack_stops_without_verified_receipt_or_replay(self):
        for fault in ("initial_drift", "staged_settings_drift", "final_drift", "lost_first_ack", "lost_marker_ack"):
            with self.subTest(fault=fault):
                result = self.run_serial(fault=fault)
                self.assertEqual(result["code"], 1)
                self.assertEqual(result["session"]["status"], "stopped")
                self.assertEqual(result["report"]["status"], "stopped")
                self.assertFalse(TOOL.COORDINATOR.inspect_journal(
                    result["output"] / "transaction/journal.jsonl")["verified_terminal"])
                if fault == "initial_drift":
                    self.assertEqual(result["peer"].calls, [])
                if fault == "lost_first_ack":
                    self.assertEqual(sum(c["name"] == "program" for c in result["peer"].calls), 128)
                if fault == "lost_marker_ack":
                    self.assertEqual(len(result["activation_observations"]), 1)

    def test_marker_acceptance_erases_once_and_commits_separately_after_full_staging_check(self):
        result = self.run_serial(current=self.baseline, marker_test=True)
        self.assertEqual(result["code"], 0)
        self.assertEqual(len(result["snapshots"]), 3)
        staged = result["snapshots"][1]
        start = TOOL.PLAN.START
        self.assertEqual(staged[start:start + 8], b"\xff" * 8)
        self.assertEqual(staged[:start], self.baseline[:start])
        self.assertEqual(staged[start + 8:], self.baseline[start + 8:])
        self.assertEqual(result["snapshots"][2], self.baseline)
        self.assertEqual(result["peer"].snapshot(), self.baseline)
        self.assertEqual(len(result["activation_observations"]), 1)
        self.assertEqual(sum(c["name"] == "erase" for c in result["peer"].calls), 1)
        self.assertEqual(result["report"]["marker_commit"]["additional_erases"], 0)
        receipt = TOOL.COORDINATOR.inspect_journal(result["output"] / "transaction/journal.jsonl")
        self.assertTrue(receipt["verified_terminal"])
        self.assertEqual(receipt["erase_order"], [start])

    def test_marker_test_failure_never_replays_or_reports_success(self):
        for fault in ("initial_drift", "staged_settings_drift", "final_drift", "lost_first_ack", "lost_marker_ack", "protected"):
            with self.subTest(fault=fault):
                result = self.run_serial(current=self.baseline, marker_test=True, fault=fault)
                self.assertEqual(result["code"], 1)
                self.assertEqual(result["report"]["status"], "stopped")
                self.assertEqual(result["session"]["status"], "stopped")
                if fault in ("initial_drift", "protected"):
                    self.assertFalse(any(c["name"] in ("erase", "program") for c in result["peer"].calls))
                if fault == "staged_settings_drift":
                    self.assertEqual(result["peer"].snapshot()[TOOL.PLAN.START:TOOL.PLAN.START + 8], b"\xff" * 8)
                    self.assertEqual(result["activation_observations"], [])

    def test_full_bank_rewrites_every_sector_while_marker_withheld_and_preserves_current_settings(self):
        current = bytearray(self.baseline)
        current[0x3E8000] ^= 1
        current[0x650000] ^= 1
        current = bytes(current)
        result = self.run_serial(current=current, bank_test=True)
        self.assertEqual(result["code"], 0, result["session"].get("error"))
        start, end = TOOL.PLAN.START, TOOL.PLAN.END
        offsets = list(range(start, end, 4096))
        self.assertEqual(len(offsets), 336)
        erases = [c["offset"] for c in result["peer"].calls if c["name"] == "erase"]
        self.assertEqual(erases, offsets)
        self.assertEqual(result["body_erases_while_withheld"], 335)
        self.assertEqual(result["report"]["operation"], TOOL.MARKER.BANK_KIND)
        self.assertEqual(result["report"]["rewritten_sector_count"], 336)
        self.assertEqual(len(result["snapshots"]), 3)
        self.assertEqual(result["snapshots"][1], current[:start] + b"\xff" * 8 + current[start + 8:])
        self.assertEqual(result["snapshots"][2], current)
        self.assertEqual(result["peer"].snapshot(), current)
        self.assertEqual(result["activation_observations"], [True])
        self.assertEqual(result["report"]["marker_commit"]["additional_erases"], 0)
        programs = [c for c in result["peer"].calls if c["name"] == "program"]
        self.assertEqual(len(programs), (336 * 4096 + 1024) // 8)
        self.assertTrue(all(start <= p["offset"] and p["offset"] + p["bytes"] <= end for p in programs))
        self.assertEqual((result["output"] / "serial-tx.bin").read_bytes(), result["peer"].host_tx)
        expected_wire = WIRE.expected(self.baseline, include_full_reads=False)
        self.assertEqual(bytes(result["peer"].host_tx), expected_wire)
        self.assertEqual(len(expected_wire), 1_413_954)
        self.assertEqual(len(WIRE.expected(self.baseline)), 1_438_551)
        receipt = TOOL.COORDINATOR.inspect_journal(result["output"] / "transaction/journal.jsonl")
        self.assertTrue(receipt["verified_terminal"])
        self.assertEqual(receipt["erase_order"], offsets)
        self.assertFalse(result["report"]["physical_recovery_verified"])

    def test_full_bank_body_drift_protection_and_lost_erase_ack_stop_without_activation(self):
        for fault in ("body_preimage_drift", "body_protected", "lost_body_erase_ack"):
            with self.subTest(fault=fault):
                result = self.run_serial(current=self.baseline, bank_test=True, fault=fault)
                self.assertEqual(result["code"], 1)
                self.assertEqual(result["report"]["status"], "stopped")
                self.assertTrue(result["report"]["mutation_possible"])
                self.assertEqual(result["activation_observations"], [])
                self.assertEqual(result["peer"].snapshot()[TOOL.PLAN.START:TOOL.PLAN.START + 8], b"\xff" * 8)
                erases = [c["offset"] for c in result["peer"].calls if c["name"] == "erase"]
                expected = [TOOL.PLAN.START]
                if fault == "lost_body_erase_ack":
                    expected.append(TOOL.PLAN.START + 4096)
                self.assertEqual(erases, expected)
                self.assertEqual(len(result["snapshots"]), 1)
                self.assertFalse(TOOL.COORDINATOR.inspect_journal(
                    result["output"] / "transaction/journal.jsonl")["verified_terminal"])

    def test_full_bank_plan_is_exact_original_only_and_normal_recovery_stays_noop(self):
        plan = TOOL.operation_plan(self.baseline, self.baseline, TOOL.MARKER.BANK_KIND)
        self.assertEqual(plan["explicit_sector_erases"], 336)
        self.assertEqual(plan["staging_packets"], 1344)
        self.assertEqual(plan["commit_packets"], 1)
        self.assertEqual(plan["target_range"], {"start": 0x186000, "end_exclusive": 0x2D6000})
        self.assertEqual(plan["ordered_sector_payloads"][0]["offset"], 0x186000)
        self.assertEqual(plan["ordered_sector_payloads"][-1]["offset"], 0x2D5000)
        self.assertEqual([r["start"] for r in plan["preserve_current_ranges"]], [0, 0x2D6000])
        self.assertEqual(TOOL.PLAN.plan(self.baseline, self.baseline)["changed_sector_count"], 0)
        for offset in (0, 0x186000, 0x187000, 0x2D5FFF):
            altered = bytearray(self.baseline)
            altered[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                TOOL.operation_plan(self.baseline, bytes(altered), TOOL.MARKER.BANK_KIND)
        with self.assertRaisesRegex(ValueError, "boolean"):
            TOOL.MARKER.plan(self.baseline, self.baseline, full_bank=1)

    def test_full_bank_review_rejects_scope_order_payload_or_type_changes(self):
        expected = self.reviewed(self.baseline, bank_test=True)
        raw = TOOL.canonical(expected)
        self.assertEqual(TOOL.validate_reviewed(raw, TOOL.digest(raw), self.baseline, self.baseline, expected), expected)
        for mutation in ("order", "payload", "range", "boolean", "count", "kind"):
            reviewed = copy.deepcopy(expected)
            plan = reviewed["recovery_plan"]
            if mutation == "order":
                plan["ordered_sector_payloads"].reverse()
            elif mutation == "payload":
                plan["ordered_sector_payloads"][-1]["stage_sha256"] = "0" * 64
            elif mutation == "range":
                plan["target_range"]["end_exclusive"] += 4096
            elif mutation == "boolean":
                plan["commit_packets"] = True
            elif mutation == "count":
                plan["explicit_sector_erases"] -= 1
            else:
                reviewed["kind"] = TOOL.MARKER.MARKER_KIND
            raw = TOOL.canonical(reviewed)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                TOOL.validate_reviewed(raw, TOOL.digest(raw), self.baseline, self.baseline, expected)

    def test_full_bank_cli_requires_its_own_explicit_execution_mode(self):
        expected = self.reviewed(self.baseline, bank_test=True)
        raw = TOOL.canonical(expected)
        for mode in ("execute", "activation-test", "bank-writeback-test"):
            argv = ["flash/uart-stock-recovery.py", mode, "--baseline", "unused", "--current", "unused",
                    "--rom-report", "unused", "--small-report", "unused", "--output", "unused",
                    "--port", "/fixture", "--plan", "unused", "--expected-plan-sha256", TOOL.digest(raw)]
            with self.subTest(mode=mode), patch.object(sys, "argv", argv), patch.object(TOOL.os, "umask"), \
                    patch.object(TOOL, "prepare_inputs", return_value=(self.baseline, self.baseline, expected)), \
                    patch.object(TOOL, "private_file", return_value=raw), patch.object(TOOL, "execute", return_value=0) as execute:
                if mode == "bank-writeback-test":
                    self.assertEqual(TOOL.main(), 0)
                    execute.assert_called_once()
                else:
                    with self.assertRaisesRegex(ValueError, "execution mode"):
                        TOOL.main()
                    execute.assert_not_called()

    def test_marker_test_rejects_nonoriginal_bank_factory_or_altered_plan(self):
        for offset in (0, TOOL.PLAN.START, TOOL.PLAN.START + 4096):
            current = bytearray(self.baseline)
            current[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                TOOL.MARKER.plan(self.baseline, bytes(current))
        expected = self.reviewed(self.baseline, marker_test=True)
        altered = copy.deepcopy(expected)
        altered["recovery_plan"]["commit_packets"] = True
        raw = TOOL.canonical(altered)
        with self.assertRaises(ValueError):
            TOOL.validate_reviewed(raw, TOOL.digest(raw), self.baseline, self.baseline, expected)

    def test_truncated_snapshot_retains_partial_and_never_mutates(self):
        result = self.run_serial(fault="truncated_snapshot")
        self.assertEqual(result["code"], 1)
        self.assertEqual(result["peer"].calls, [])
        self.assertEqual(result["session"]["snapshots"], [])
        self.assertTrue((result["output"] / "snapshot-01.bin.part").is_file())
        self.assertFalse((result["output"] / "snapshot-01.bin").exists())
        self.assertFalse(result["report"]["mutation_possible"])

    def test_preparation_checks_full_snapshot_and_raw_session_provenance(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            private = root / "private"
            bounded, full = private / "bounded", private / "full"
            bounded.mkdir(parents=True)
            full.mkdir()
            (root / "tools").symlink_to(ROOT / "tools", target_is_directory=True)
            baseline_path = private / "baseline.bin"
            baseline_path.write_bytes(self.baseline)
            (full / "flash-read.bin").write_bytes(self.current)
            (bounded / "flash-read.bin").write_bytes(self.current[:4096])
            small = {"status": "complete", "mode": "read-small", "port": "/fixture",
                     "helper_sha256": TOOL.READER.HELPER_SHA256, "helper_upload_completed": True,
                     "result": {"bytes": 4096, "offset": 0, "sha256": TOOL.digest(self.current[:4096])}}
            report = {"status": "complete", "mode": "read-full", "port": "/fixture",
                      "helper_sha256": TOOL.READER.HELPER_SHA256, "helper_upload_attempted": False,
                      "result": {"bytes": len(self.current), "offset": 0, "sha256": TOOL.digest(self.current)}}
            for directory, meta in ((bounded, small), (full, report)):
                for name in ("serial-rx.bin", "serial-tx.bin"):
                    (directory / name).write_bytes(b"fixture evidence")
                    meta[name + "_sha256"] = TOOL.digest(b"fixture evidence")
            small_path = bounded / "report.json"
            small_path.write_text(json.dumps(small))
            report["bounded_read_report_sha256"] = TOOL.digest(small_path.read_bytes())
            full_path = full / "report.json"
            full_path.write_text(json.dumps(report))
            rom = private / "rom.json"
            rom.write_text('{}')
            args = SimpleNamespace(mode="prepare", baseline=baseline_path, current=full / "flash-read.bin",
                                   small_report=small_path, rom_report=rom, port="/fixture")
            with patch.object(TOOL, "ROOT", root), patch.object(TOOL.READER, "ROOT", root), \
                    patch.object(TOOL.READER, "load_helper", return_value=self.helper), \
                    patch.object(TOOL.READER, "validate_rom_report"):
                _, _, reviewed = TOOL.prepare_inputs(args)
                self.assertEqual(reviewed["kind"], "original_stock_bank1_recovery")
                self.assertEqual(len(reviewed["tool_sha256"]), len(TOOL.SOURCES))
                (full / "flash-read.bin").write_bytes(self.baseline)
                stock_report = copy.deepcopy(report)
                stock_report["result"]["sha256"] = TOOL.digest(self.baseline)
                full_path.write_text(json.dumps(stock_report))
                args.mode = "prepare-bank-writeback-test"
                _, _, bank_reviewed = TOOL.prepare_inputs(args)
                self.assertEqual(bank_reviewed["kind"], TOOL.MARKER.BANK_KIND)
                self.assertEqual(bank_reviewed["recovery_plan"]["explicit_sector_erases"], 336)
                args.mode = "prepare"
                (full / "flash-read.bin").write_bytes(self.current)
                for key, wrong in (("port", "/other"), ("helper_upload_attempted", True),
                                   ("bounded_read_report_sha256", "0" * 64)):
                    full_path.write_text(json.dumps({**report, key: wrong}))
                    with self.subTest(key=key), self.assertRaises(ValueError):
                        TOOL.prepare_inputs(args)
                full_path.write_text(json.dumps(report))
                (full / "serial-rx.bin").write_bytes(b"altered")
                with self.assertRaisesRegex(ValueError, "serial evidence hash mismatch"):
                    TOOL.prepare_inputs(args)


if __name__ == "__main__":
    unittest.main()
