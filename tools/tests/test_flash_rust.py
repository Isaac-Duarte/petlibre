"""The Rust flash client (crates/petlibre-flash) against the Python tools.

Without fixtures: status decoding and journal hashing agree with the Python
modules. With fixtures: the binary drives the pinned vendor helper under
emulation, and its plan, wire and journal match the Python planner, the
independent wire policy and the Python journal inspector. No device opens.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import unittest
from support import evidence as requires_evidence

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target/debug/petlibre-flash"


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


STATUS = module("rust_flash_status", "flash/flash-status.py")
C = module("rust_flash_coordinator", "flash/recovery-transaction.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"
SOURCE = ROOT / "private/fixtures/development-source"
TARGET = ROOT / "private/fixtures/development-target"
SMALL = ROOT / "private/fixtures/development-bounded-read/report.json"
ROM = ROOT / "private/fixtures/rom-inspection/report.json"
HELPER = ROOT / "private/upstream/ameba-rtos-d/tools/AmebaD/Image_Tool/imgtool_flashloader_amebad.bin"


def binary():
    """Build the client once if `make check-rust` has not already."""
    if not BINARY.is_file():
        subprocess.run(["cargo", "build", "--locked", "-q", "-p", "petlibre-flash"], cwd=ROOT, check=True)
    return BINARY


def run(*args, env=None):
    result = subprocess.run([str(binary()), "--root", str(ROOT), *map(str, args)], cwd=ROOT,
                            capture_output=True, text=True, env=env, timeout=600)
    lines = result.stdout.strip().splitlines()
    return result.returncode, json.loads(lines[-1]) if lines else None, result.stderr


class Emulator:
    """Serve one emulated helper session on a Unix socket, one client at a time.

    Requests are an op byte and a little-endian u32: W writes that many
    following bytes, R reads up to that many (running the helper until it has
    output), I reports buffered output, F flushes.
    """

    def __init__(self, peer):
        self.peer, self.error = peer, None
        self.directory = tempfile.TemporaryDirectory(prefix="pf-")
        self.path = Path(self.directory.name) / "helper.sock"
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(self.path))
        self.listener.listen(1)
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def env(self):
        return {**os.environ, "PETLIBRE_FLASH_EMULATOR": str(self.path)}

    @staticmethod
    def exact(connection, count):
        data = bytearray()
        while len(data) < count:
            chunk = connection.recv(count - len(data))
            if not chunk:
                return None
            data.extend(chunk)
        return bytes(data)

    def serve(self):
        while True:
            try:
                connection, _ = self.listener.accept()
            except OSError:
                return
            with connection:
                try:
                    while (header := self.exact(connection, 5)) is not None:
                        op, argument = header[0], int.from_bytes(header[1:], "little")
                        if op == ord("W"):
                            reply = self.peer.write(self.exact(connection, argument)).to_bytes(4, "little")
                        elif op == ord("R"):
                            reply = self.peer.read(argument)
                        elif op == ord("I"):
                            reply = self.peer.in_waiting.to_bytes(4, "little")
                        else:
                            reply = bytes(4)
                        connection.sendall(len(reply).to_bytes(4, "little") + reply)
                except Exception as error:  # The client sees a closed transport and stops.
                    self.error = error

    def close(self):
        self.listener.close()
        self.directory.cleanup()


class StatusAndJournalAgreement(unittest.TestCase):
    def test_status_decoding_matches_flash_status_py(self):
        for sr1 in range(256):
            for sr2, sr3 in ((0, 0), (64, 0x61), (0x86, 0x9e)):
                code, rust, _ = run("decode-status", "--sr1", sr1, "--sr2", sr2, "--sr3", sr3)
                self.assertEqual(code, 0)
                self.assertEqual(rust, STATUS.assess_range(sr1, sr2, sr3, 0x186000, 0x2D6000), (sr1, sr2))

    def test_python_journals_inspect_identically(self):
        (ROOT / "private").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / "private") as directory:
            journal = C.Journal(Path(directory) / "transaction")
            journal.append({"event": "transaction_start", "transaction_id": "t", "text": "é\"\n\x01"})
            journal.append({"event": "erase_intent", "offset": 0x186000, "bytes": 4096})
            journal.append({"event": "transaction_verified", "snapshot_sha256": "0" * 64})
            journal.close()
            path = Path(directory) / "transaction/journal.jsonl"
            code, rust, _ = run("inspect-journal", path)
            self.assertEqual(code, 0)
            self.assertEqual(rust, C.inspect_journal(path))
            data = path.read_bytes()
            path.write_bytes(data.replace(b"1597440", b"1597441"))
            self.assertEqual(run("inspect-journal", path)[0], 2)


@requires_evidence(all(p.exists() for p in (BASELINE, SOURCE / "review.json", TARGET / "review.json",
                                            SMALL, ROM, HELPER)),
                   "Requires pinned backup, frozen reviews, session reports and the pinned helper")
class EmulatedHelperTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.SIM = module("rust_flash_peer", "emulate/emulate-recovery-session.py")
        cls.CLI = module("rust_flash_cli", "flash/uart-development.py")
        cls.WIRE = module("rust_flash_wire", "flash/development-wire-policy.py")
        cls.port = json.loads(SMALL.read_text())["port"]

    def temporary(self):
        directory = tempfile.TemporaryDirectory(dir=ROOT / "private")
        self.addCleanup(directory.cleanup)
        return Path(directory.name)

    def unit(self):
        B = self.CLI.B
        baseline = BASELINE.read_bytes()
        source, _ = self.CLI.frozen(SOURCE)
        target, _ = self.CLI.frozen(TARGET)
        full = bytearray(baseline)
        full[B.START:B.START + len(B.payload(source))] = B.payload(source)
        full[0x650000] ^= 1
        length = B.extent(source, target)
        sectors = list(range(B.START, B.START + length, B.SECTOR))
        peer = self.SIM.EmulatedHelperPort(HELPER.read_bytes(), bytes(full), sectors, status_registers=(0, 2, 0))
        emulator = Emulator(peer)
        self.addCleanup(emulator.close)
        return emulator, bytes(full), source, target, length

    def emulated(self, report):
        """A copy of a real session report marked as emulator evidence, which
        is all an emulated session may continue."""
        directory = self.temporary() / report.parent.name
        shutil.copytree(report.parent, directory)
        value = json.loads(report.read_text())
        value["transport"] = "emulator"
        (directory / report.name).write_text(json.dumps(value))
        return directory / report.name

    def evidence(self, output):
        if not hasattr(self, "rom"):
            self.rom, self.small = self.emulated(ROM), self.emulated(SMALL)
        return ["--port", self.port, "--baseline", BASELINE, "--rom-report", self.rom,
                "--small-report", self.small, "--output", output]

    def test_serial_evidence_never_continues_an_emulated_session(self):
        emulator, *_ = self.unit()
        root = self.temporary()
        code, summary, _ = run("update", "inspect", "--port", self.port, "--baseline", BASELINE,
                               "--rom-report", ROM, "--small-report", SMALL, "--output", root / "inspect",
                               "--source", SOURCE, "--target", TARGET, env=emulator.env())
        self.assertEqual((code, summary["status"]), (2, "refused"))
        self.assertIn("different transport", summary["error"])
        self.assertFalse(emulator.peer.host_tx)

    def test_update_plans_writes_and_journals_like_the_python_client(self):
        B = self.CLI.B
        emulator, before, source, target, length = self.unit()
        root = self.temporary()
        images = ["--source", SOURCE, "--target", TARGET]
        code, summary, stderr = run("update", "inspect", *self.evidence(root / "inspect"), *images,
                                    env=emulator.env())
        self.assertEqual((code, summary["status"]), (0, "complete"), stderr)
        report = json.loads((root / "inspect/report.json").read_text())
        self.assertEqual(report["transport"], "emulator")
        self.assertEqual(report["status_checks"][0]["event"], "flash_status_checked")
        code, summary, stderr = run("update", "prepare", *self.evidence(root / "plan.json"), *images,
                                    "--inspection", root / "inspect/report.json", env=emulator.env())
        self.assertEqual(code, 0, stderr)
        plan = json.loads((root / "plan.json").read_text())
        current = before[B.START:B.START + length]
        other = before[0x6000:0x7000]
        self.assertGreater(len(plan["operation_plan"]["ordered_sectors"]), 1)
        # The Rust plan is the Python planner's, byte for byte.
        self.assertEqual(C.canonical(plan["operation_plan"]), C.canonical(B.plan(source, target, current, other=other)))
        self.assertEqual(hashlib.sha256((root / "plan.json").read_bytes()).hexdigest(), summary["plan_sha256"])
        emulator.peer.host_tx.clear()
        code, summary, stderr = run("update", "execute", *self.evidence(root / "execute"), *images,
                                    "--inspection", root / "inspect/report.json", "--plan", root / "plan.json",
                                    "--expected-plan-sha256", summary["plan_sha256"], env=emulator.env())
        self.assertEqual((code, summary["status"]), (0, "verified"), stderr)
        self.assertIsNone(emulator.error)
        tx = (root / "execute/serial-tx.bin").read_bytes()
        self.assertEqual(tx, self.WIRE.expected(source, target, current))
        self.assertEqual(bytes(emulator.peer.host_tx), tx)
        erases = [c for c in emulator.peer.calls if c["name"] == "erase"]
        self.assertEqual([c["offset"] for c in erases], [s["offset"] for s in plan["operation_plan"]["ordered_sectors"]])
        self.assertNotEqual(B.payload(source), B.payload(target))
        after = emulator.peer.snapshot()
        self.assertEqual(after[:B.START], before[:B.START])
        self.assertEqual(after[B.START + length:], before[B.START + length:])
        self.assertEqual(after[B.START:B.START + len(B.payload(target))], B.payload(target))
        receipt = C.inspect_journal(root / "execute/transaction/journal.jsonl")
        self.assertTrue(receipt["verified_terminal"])
        self.assertEqual(receipt["erase_order"], [s["offset"] for s in plan["operation_plan"]["ordered_sectors"]])
        # A second execution of the same plan is refused before serial access.
        code, summary, _ = run("update", "execute", *self.evidence(root / "again"), *images,
                               "--inspection", root / "inspect/report.json", "--plan", root / "plan.json",
                               "--expected-plan-sha256", "0" * 64, env=emulator.env())
        self.assertEqual((code, summary["status"]), (2, "refused"))
        self.assertFalse((root / "again").exists())

    def test_give_way_then_restore_marker_through_the_cli(self):
        B = self.CLI.B
        emulator, before, source, _, _ = self.unit()
        # Bank 0 holds exactly the reviewed fallback; bank 1 the reviewed source.
        fallback = B.payload(self.CLI.frozen(TARGET, B.BANK0)[0], B.BANK0)
        flash = bytearray(before)
        flash[0x6000:0x6000 + len(fallback)] = fallback
        emulator.peer.uc.mem_write(0x08000000, bytes(flash))
        root = self.temporary()
        give = ["--fallback", TARGET]
        code, _, stderr = run("give-way", "inspect", *self.evidence(root / "inspect"), *give, env=emulator.env())
        self.assertEqual(code, 0, stderr)
        code, summary, stderr = run("give-way", "prepare", *self.evidence(root / "plan.json"), *give,
                                    "--inspection", root / "inspect/report.json", env=emulator.env())
        self.assertEqual(code, 0, stderr)
        code, summary, stderr = run("give-way", "execute", *self.evidence(root / "execute"), *give,
                                    "--inspection", root / "inspect/report.json", "--plan", root / "plan.json",
                                    "--expected-plan-sha256", summary["plan_sha256"], env=emulator.env())
        self.assertEqual((code, summary["status"]), (0, "verified"), stderr)
        after = emulator.peer.snapshot()
        self.assertEqual(after[B.START], 0)
        self.assertEqual(after[B.START + 1:], bytes(flash)[B.START + 1:])
        self.assertEqual(after[:B.START], bytes(flash)[:B.START])
        self.assertFalse([c for c in emulator.peer.calls if c["name"] == "erase"])
        # Restoring bank 1 is a development update of the source onto itself.
        images = ["--source", SOURCE, "--target", SOURCE, "--restore-marker"]
        code, _, stderr = run("update", "inspect", *self.evidence(root / "restore-inspect"), *images,
                              env=emulator.env())
        self.assertEqual(code, 0, stderr)
        code, summary, stderr = run("update", "prepare", *self.evidence(root / "restore.json"), *images,
                                    "--inspection", root / "restore-inspect/report.json", env=emulator.env())
        self.assertEqual(code, 0, stderr)
        code, summary, stderr = run("update", "execute", *self.evidence(root / "restore"), *images,
                                    "--inspection", root / "restore-inspect/report.json",
                                    "--plan", root / "restore.json", "--expected-plan-sha256",
                                    summary["plan_sha256"], env=emulator.env())
        self.assertEqual((code, summary["status"]), (0, "verified"), stderr)
        self.assertEqual(emulator.peer.snapshot(), bytes(flash))

    def test_status_read_continues_the_session_with_whitelisted_reads_only(self):
        emulator, *_ = self.unit()
        root = self.temporary()
        rom, small = self.emulated(ROM), self.emulated(SMALL)
        code, summary, stderr = run("read", "status", "--port", self.port, "--rom-report", rom,
                                    "--small-report", small, "--helper", HELPER, "--output", root / "status",
                                    env=emulator.env())
        self.assertEqual((code, summary["status"]), (0, "complete"), stderr)
        report = json.loads((root / "status/report.json").read_text())
        self.assertEqual(report["status_observations"], [[0, 2, 0], [0, 2, 0]])
        self.assertTrue(report["result"]["bank1_assessment"]["software_preconditions_met"])
        self.assertFalse(report["helper_upload_attempted"])
        self.assertEqual((root / "status/serial-tx.bin").read_bytes(),
                         b"".join(bytes([0x21, op, 1]) for _ in range(2) for op in (5, 0x35, 0x15)))


if __name__ == "__main__":
    unittest.main()
