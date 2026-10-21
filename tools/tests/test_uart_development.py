"""CLI evidence, immutable plans and actual helper region-transfer integration."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import shutil
import sys
import tempfile
from types import SimpleNamespace
import unittest
from support import evidence as requires_evidence
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


CLI = module("uart_development_test", "uart-development.py")
REVIEW = module("review_development_test", "review-development-image.py")
SIM = module("uart_development_peer", "emulate-recovery-session.py")
WIRE = module("uart_development_wire", "development-wire-policy.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"
SOURCE = ROOT / "private/fixtures/development-source"
TARGET = ROOT / "private/fixtures/development-target"
SMALL = ROOT / "private/fixtures/development-bounded-read/report.json"
ROM = ROOT / "private/fixtures/rom-inspection/report.json"


@requires_evidence(all(p.exists() for p in (BASELINE, SOURCE / "review.json", TARGET / "review.json", SMALL, ROM)),
                     "Requires pinned backup and frozen old/new build reviews")
class UartDevelopmentTests(unittest.TestCase):
    def temporary(self):
        temporary = tempfile.TemporaryDirectory(dir=ROOT / "private")
        self.addCleanup(temporary.cleanup)
        return Path(temporary.name)

    def args(self, output, mode="inspect"):
        return SimpleNamespace(mode=mode, baseline=BASELINE, source=SOURCE, target=TARGET,
                               rom_report=ROM, small_report=SMALL, port="/dev/ttyUSB0", output=output,
                               inspection=None, plan=None, expected_plan_sha256=None, interrupted=False)

    def test_frozen_review_binds_ota_manifest_and_required_audits(self):
        original = CLI.frozen(TARGET)[0]
        for fault in ("image", "manifest", "review", "missing_mapping", "bool_size"):
            with self.subTest(fault=fault):
                directory = self.temporary() / "review"
                shutil.copytree(TARGET, directory)
                if fault == "image":
                    (directory / "image.bin").write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
                elif fault == "manifest":
                    (directory / "build-manifest.json").write_text("{}")
                else:
                    p = directory / "review.json"
                    d = json.loads(p.read_text())
                    if fault == "review":
                        d["ota_sha256"] = "0" * 64
                    elif fault == "bool_size":
                        d["payload_bytes"] = True
                    else:
                        d["review"]["factory_mapping"] = []
                    p.write_text(json.dumps(d))
                with self.assertRaises(ValueError):
                    CLI.frozen(directory)

    def test_unknown_or_stale_build_refused_before_freezing(self):
        root = self.temporary()
        build = root / "build"
        build.mkdir()
        (build / "manifest.json").write_text(json.dumps({"profile": "bringup"}))
        with self.assertRaisesRegex(ValueError, "startup profile"):
            REVIEW.freeze("build", build, root / "output")
        self.assertFalse((root / "output").exists())

    def inspect(self, root):
        args = self.args(root / "inspect")
        baseline, source, target, bindings = CLI.inputs(args)
        full = bytearray(baseline)
        full[CLI.B.START:CLI.B.START + len(CLI.B.payload(source))] = CLI.B.payload(source)
        full[0x650000] ^= 1
        current = bytes(full[CLI.B.START:CLI.B.START + CLI.B.extent(source, target)])
        planned = CLI.B.plan(source, target, current)
        peer = SIM.EmulatedHelperPort(SIM.HELPER.read_bytes(), bytes(full),
                                     [x["offset"] for x in planned["ordered_sectors"]] or [CLI.B.START],
                                     status_registers=(0, 2, 0))
        peer.open = peer.close = lambda: None
        serial = SimpleNamespace(Serial=lambda **_: peer)
        with patch.dict(sys.modules, {"serial": serial}), patch("sys.stdout", new_callable=io.StringIO):
            self.assertEqual(CLI.session(args, baseline, source, target, bindings), 0)
        args.mode = "execute"
        args.inspection = args.output / "report.json"
        args.output = root / "execute"
        return args, peer, serial, bytes(full)

    def test_inspection_plan_and_execute_match_independent_actual_region_wire(self):
        root = self.temporary()
        args, peer, serial, before = self.inspect(root)
        baseline, source, target, current, reviewed = CLI.prepared(args)
        raw = CLI.canonical(reviewed) + b"\n"
        args.expected_plan_sha256 = CLI.digest(raw)
        bindings = {k: v for k, v in reviewed.items() if k not in ("inspection_report_sha256", "operation_plan")}
        with patch.dict(sys.modules, {"serial": serial}), patch("sys.stdout", new_callable=io.StringIO):
            self.assertEqual(CLI.session(args, baseline, source, target, bindings, current, reviewed, raw), 0)
        report = json.loads((args.output / "report.json").read_text())
        self.assertEqual(report["status"], "verified")
        self.assertEqual(len(report["observations"]), 3)
        self.assertFalse(report["helper_upload_attempted"])
        self.assertFalse(report["application_boot_verified"])
        for observation in report["observations"]:
            self.assertEqual([(x["offset"], x["bytes"]) for x in observation],
                             [*CLI.B.GUARDS, (CLI.B.START, len(current))])
        tx = (args.output / "serial-tx.bin").read_bytes()
        self.assertEqual(tx, WIRE.expected(source, target, current))
        for name in ("serial-rx.bin", "serial-tx.bin"):
            self.assertEqual(CLI.digest((args.output / name).read_bytes()), report[name + "_sha256"])
        after = peer.snapshot()
        self.assertEqual(after[:CLI.B.START], before[:CLI.B.START])
        self.assertEqual(after[CLI.B.START + len(current):], before[CLI.B.START + len(current):])
        self.assertEqual(after[CLI.B.START:CLI.B.START + len(CLI.B.payload(target))], CLI.B.payload(target))
        self.assertTrue(CLI.C.inspect_journal(args.output / "transaction/journal.jsonl")["verified_terminal"])

    def test_interrupted_update_inspects_plans_and_finishes_through_the_cli(self):
        root = self.temporary()
        args = self.args(root / "inspect")
        args.interrupted = True
        baseline, source, target, bindings = CLI.inputs(args)
        self.assertTrue(bindings["interrupted"])
        full = bytearray(baseline)
        full[CLI.B.START:CLI.B.START + len(CLI.B.payload(source))] = CLI.B.payload(source)
        size = CLI.B.extent(source, target)
        _, staged = CLI.B.images(source, target, bytes(full[CLI.B.START:CLI.B.START + size]))
        normal = CLI.B.plan(source, target, bytes(full[CLI.B.START:CLI.B.START + size]))
        offsets = [x["offset"] for x in normal["ordered_sectors"]]
        self.assertGreater(len(offsets), 1)
        # Stopped after staging the marker, with the next sector's erase uncertain.
        full[CLI.B.START:CLI.B.START + CLI.B.SECTOR] = staged[:CLI.B.SECTOR]
        full[offsets[1]:offsets[1] + CLI.B.SECTOR] = b"\xff" * CLI.B.SECTOR
        before = bytes(full)
        peer = SIM.EmulatedHelperPort(SIM.HELPER.read_bytes(), before, offsets, status_registers=(0, 2, 0))
        peer.open = peer.close = lambda: None
        serial = SimpleNamespace(Serial=lambda **_: peer)
        with patch.dict(sys.modules, {"serial": serial}), patch("sys.stdout", new_callable=io.StringIO):
            self.assertEqual(CLI.session(args, baseline, source, target, bindings), 0)
        args.mode, args.inspection, args.output = "execute", args.output / "report.json", root / "execute"
        # The same inspection cannot prepare a normal update, nor the reverse.
        args.interrupted = False
        with self.assertRaisesRegex(ValueError, "matching inspection"):
            CLI.prepared(args)
        args.interrupted = True
        baseline, source, target, current, reviewed = CLI.prepared(args)
        self.assertTrue(reviewed["operation_plan"]["interrupted"])
        self.assertEqual([x["offset"] for x in reviewed["operation_plan"]["ordered_sectors"]], offsets[1:])
        raw = CLI.canonical(reviewed) + b"\n"
        args.expected_plan_sha256 = CLI.digest(raw)
        bindings = {k: v for k, v in reviewed.items() if k not in ("inspection_report_sha256", "operation_plan")}
        with patch.dict(sys.modules, {"serial": serial}), patch("sys.stdout", new_callable=io.StringIO):
            self.assertEqual(CLI.session(args, baseline, source, target, bindings, current, reviewed, raw), 0)
        self.assertEqual(json.loads((args.output / "report.json").read_text())["status"], "verified")
        after = peer.snapshot()
        self.assertEqual(after[:CLI.B.START], before[:CLI.B.START])
        self.assertEqual(after[CLI.B.START + size:], before[CLI.B.START + size:])
        self.assertEqual(after[CLI.B.START:CLI.B.START + len(CLI.B.payload(target))], CLI.B.payload(target))
        self.assertTrue(CLI.C.inspect_journal(args.output / "transaction/journal.jsonl")["verified_terminal"])

    def test_changed_inspection_or_tools_cannot_prepare(self):
        root = self.temporary()
        args, peer, _serial, _ = self.inspect(root)
        before = bytes(peer.host_tx)
        r = json.loads(args.inspection.read_text())
        r["bindings"]["tool_sha256"]["development-bank.py"] = "0" * 64
        args.inspection.write_text(json.dumps(r))
        with self.assertRaisesRegex(ValueError, "matching inspection"):
            CLI.prepared(args)
        self.assertEqual(bytes(peer.host_tx), before)

    def test_changed_plan_hash_or_scope_and_reused_output_refuse_before_open(self):
        root = self.temporary()
        args, _peer, _serial, _ = self.inspect(root)
        baseline, source, target, current, reviewed = CLI.prepared(args)
        bindings = {k: v for k, v in reviewed.items() if k not in ("inspection_report_sha256", "operation_plan")}
        never = SimpleNamespace(Serial=lambda **_: self.fail("Serial opened before refusal"))
        with patch.dict(sys.modules, {"serial": never}):
            for fault in ("hash", "scope", "type"):
                d = copy.deepcopy(reviewed)
                if fault == "scope":
                    d["operation_plan"]["bank"] = 0
                if fault == "type":
                    d["operation_plan"]["commit_packets"] = True
                raw = CLI.canonical(d)
                args.expected_plan_sha256 = "0" * 64 if fault == "hash" else CLI.digest(raw)
                with self.subTest(fault=fault), self.assertRaises(ValueError):
                    CLI.session(args, baseline, source, target, bindings, current, d, raw)
            raw = CLI.canonical(reviewed)
            args.expected_plan_sha256 = CLI.digest(raw)
            args.output.mkdir()
            with self.assertRaises(FileExistsError):
                CLI.session(args, baseline, source, target, bindings, current, reviewed, raw)


if __name__ == "__main__":
    unittest.main()
