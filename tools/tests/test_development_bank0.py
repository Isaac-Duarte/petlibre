"""Bank-0 development writes against actual helper instructions; no device I/O."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from support import evidence as requires_evidence

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


B = module("development_bank0_test", "flash/development-bank.py")
SIM = module("development_bank0_peer", "emulate/emulate-recovery-session.py")
W = module("development_bank0_wire", "flash/development-wire-policy.py")
BASELINE = ROOT / "private/fixtures/baseline.bin"
SEED = ROOT / "private/fixtures/diagnostic-ota.bin"
L0 = B.BANK0


class FrameBounds(unittest.TestCase):
    def test_frames_stay_inside_their_bank_and_never_reach_the_factory_region(self):
        data = b"\0" * 1024
        self.assertEqual(B.frame(L0, 1, 0x6000, data)[3:7], (0x08006000).to_bytes(4, "little"))
        for offset in (0, 0x5c00, 0x156000, 0x186000):
            with self.subTest(offset=hex(offset)), self.assertRaises(ValueError):
                B.frame(L0, 1, offset, data)
        self.assertEqual(B.frame(B.BANK1, 1, 0x186000, data), B.T.program_frame(1, 0x186000, data))
        with self.assertRaises(ValueError):
            B.frame(B.BANK1, 1, 0x6000, data)
        with self.assertRaises(ValueError):
            W.expected(None, b"", b"", bank=2)


@requires_evidence(BASELINE.is_file() and SEED.is_file() and SIM.HELPER.is_file(),
                   "Requires original backup, diagnostic seed and pinned helper")
class Bank0Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline, cls.seed, cls.helper = BASELINE.read_bytes(), SEED.read_bytes(), SIM.HELPER.read_bytes()

    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=ROOT / "private")
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name) / "transaction"

    def run_overwrite(self, full=None, tamper=None):
        full = bytes(full or self.baseline)
        target = self.seed
        length = len(B.payload(target, L0))
        current = full[L0.start:L0.start + length]
        other = full[L0.other[0]:L0.other[0] + L0.other[1]]
        original = B.original_region(self.baseline, target, L0)  # The baseline serves as the pinned read.
        planned = B.plan(None, target, current, False, L0, other, original)
        offsets = [e["offset"] for e in planned["ordered_sectors"]]
        marker_states = []

        class Peer(SIM.EmulatedHelperPort):
            def _step(self, uc, pc, size, data):
                if pc == 0x2146:  # Erase: bank 0's marker stays withheld meanwhile.
                    marker_states.append(bytes(uc.mem_read(0x08000000 + L0.start, 8)))
                super()._step(uc, pc, size, data)

        peer = Peer(self.helper, full, offsets, status_registers=(0, 2, 0), scope=(L0.start, L0.end))
        count = {"n": 0}

        def acquire():
            count["n"] += 1
            snapshot = peer.snapshot()
            protected = [snapshot[o:o + n] for o, n in (B.FACTORY, L0.other)]
            if tamper == "other_bank" and count["n"] == 2:
                # Still a valid marker, but not the sector the plan pinned.
                protected[1] = protected[1][:100] + bytes([protected[1][100] ^ 1]) + protected[1][101:]
            return snapshot[L0.start:L0.start + length], protected

        result = B.run(peer, self.baseline, None, target, current, planned, acquire, self.output, layout=L0, other=other, original=original)
        return result, peer, full, planned, marker_states

    def test_overwrite_writes_bank_0_with_its_marker_last_and_touches_nothing_else(self):
        result, peer, before, planned, markers = self.run_overwrite()
        self.assertEqual(result["status"], "verified_via_supplied_transports")
        self.assertEqual(planned["kind"], "plaf103_bank0_development_update")
        self.assertIsNone(planned["source_ota_sha256"])
        after = peer.snapshot()
        payload = B.payload(self.seed, L0)
        self.assertEqual(after[L0.start:L0.start + len(payload)], payload)
        # Factory region, bank 1 and everything past the target are unchanged.
        self.assertEqual(after[:L0.start], before[:L0.start])
        self.assertEqual(after[L0.start + len(payload):], before[L0.start + len(payload):])
        # Bank 0 was never valid before the last packet.
        self.assertTrue(all(m[:8] != B.MARKER for m in markers))
        self.assertEqual(bytes(peer.host_tx), W.expected(None, self.seed, before[L0.start:L0.start + len(payload)],
                                                         include_acquisitions=False, bank=0))

    def test_bank_1_must_be_valid_and_stay_unchanged(self):
        invalid = bytearray(self.baseline)
        invalid[B.START] = 0
        with self.assertRaisesRegex(ValueError, "Bank 1 must be valid"):
            B.guards(self.baseline, [bytes(invalid[:0x6000]), bytes(invalid[B.START:B.START + B.SECTOR])], L0)
        with self.assertRaisesRegex(ValueError, "Other bank changed since inspection"):
            self.run_overwrite(tamper="other_bank")

    def test_overwrite_refuses_a_bank_0_that_is_not_its_pinned_original(self):
        changed = bytearray(self.baseline)
        changed[L0.start + 5000] ^= 1
        with self.assertRaisesRegex(ValueError, "not the reviewed source image"):
            self.run_overwrite(full=changed)

    def test_bank_1_path_accepts_a_valid_bank_0_fallback_but_not_an_unknown_one(self):
        factory = self.baseline[:0x6000]
        valid = B.payload(self.seed)[:B.SECTOR]
        B.guards(self.baseline, [factory, valid])  # A valid bank 0 is the loader's fallback.
        B.guards(self.baseline, [factory, self.baseline[0x6000:0x6000 + B.SECTOR]])
        unknown = b"\0" + valid[1:]
        with self.assertRaisesRegex(ValueError, "neither the pinned baseline nor a valid image"):
            B.guards(self.baseline, [factory, unknown])
        with self.assertRaisesRegex(ValueError, "Factory region differs"):
            B.guards(self.baseline, [b"\0" + factory[1:], valid])


FM_SPEC = importlib.util.spec_from_file_location("bank0_factory", ROOT / "tools/audit/audit-factory-mapping.py")


def loader_choice(flash):
    """The factory loader's own selection (emulated): 'bank0', 'bank1' or 'none'."""
    fm = importlib.util.module_from_spec(FM_SPEC)
    FM_SPEC.loader.exec_module(fm)
    result = fm.execute(bytearray(flash), fm.ROM.read_bytes())
    return "none" if result["stop"] == "0x8000482" else {1: "bank0", 2: "bank1"}[result["result"]]


@requires_evidence(BASELINE.is_file() and SEED.is_file() and SIM.HELPER.is_file() and
                   (ROOT / "private/upstream/ameba-rtos-d/project/realtek_amebaD_va0_example/GCC-RELEASE/"
                    "project_lp/asdk/lib/rom_bcut/km0_irom.bin").is_file(),
                   "Requires original backup, diagnostic seed, pinned helper and SDK ROM")
class GiveWayAndRestoreTests(unittest.TestCase):
    """FW09 steps A-C, chained: bank 0 installed, bank 1 gives way, bank 1 restored."""

    @classmethod
    def setUpClass(cls):
        cls.baseline, cls.seed, cls.helper = BASELINE.read_bytes(), SEED.read_bytes(), SIM.HELPER.read_bytes()
        cls.stock = (ROOT / "private/firmware/PLAF103_OTA_010115.bin").read_bytes()

    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=ROOT / "private")
        self.addCleanup(directory.cleanup)
        self.dir = Path(directory.name)

    def test_give_way_then_restore_moves_the_loader_between_banks(self):
        # Step A's result: the seed in bank 0 (valid), stock still in bank 1.
        full = bytearray(self.baseline)
        payload0 = B.payload(self.seed, L0)
        full[L0.start:L0.start + len(payload0)] = payload0
        full = bytes(full)
        self.assertEqual(loader_choice(full), "bank1")

        bank0 = full[L0.start:L0.start + len(payload0)]
        sector = full[B.START:B.START + B.SECTOR]
        plan = B.give_way_plan(self.baseline, self.seed, bank0, sector)
        peer = SIM.EmulatedHelperPort(self.helper, full, [B.START], status_registers=(0, 2, 0))

        def guards():
            s = peer.snapshot()
            return s[:0x6000], s[L0.start:L0.start + len(payload0)], s[B.START:B.START + B.SECTOR]

        B.run_give_way(peer, self.baseline, self.seed, bank0, sector, plan, guards, self.dir / "give-way")
        after = peer.snapshot()
        self.assertEqual(after[B.START], 0)
        self.assertEqual(after[B.START + 1:], full[B.START + 1:])
        self.assertEqual(after[:B.START], full[:B.START])
        self.assertEqual(loader_choice(after), "bank0")

        # Step C: restore bank 1 to stock (a one-sector update, marker last).
        length = len(B.payload(self.stock))
        current = after[B.START:B.START + length]
        other = after[0x6000:0x6000 + B.SECTOR]
        planned = B.plan(self.stock, self.stock, current, layout=B.BANK1, other=other, restore=True)
        self.assertEqual([e["offset"] for e in planned["ordered_sectors"]], [B.START])
        peer2 = SIM.EmulatedHelperPort(self.helper, after, [B.START], status_registers=(0, 2, 0))

        def acquire():
            s = peer2.snapshot()
            return s[B.START:B.START + length], [s[:0x6000], s[0x6000:0x6000 + B.SECTOR]]

        B.run(peer2, self.baseline, self.stock, self.stock, current, planned, acquire, self.dir / "restore",
              layout=B.BANK1, other=other, restore=True)
        restored = peer2.snapshot()
        self.assertEqual(restored, full)
        self.assertEqual(loader_choice(restored), "bank1")

    def test_give_way_refuses_without_an_exact_reviewed_bank_0(self):
        sector = self.baseline[B.START:B.START + B.SECTOR]
        # The original bank 0 (invalidated 1.1.16 body) is not the reviewed fallback.
        original = self.baseline[L0.start:L0.start + len(B.payload(self.seed, L0))]
        with self.assertRaisesRegex(ValueError, "not exactly the reviewed fallback"):
            B.give_way_plan(self.baseline, self.seed, original, sector)
        damaged = bytearray(B.payload(self.seed, L0))
        damaged[9000] ^= 1
        with self.assertRaisesRegex(ValueError, "not exactly the reviewed fallback"):
            B.give_way_plan(self.baseline, self.seed, bytes(damaged), sector)
        with self.assertRaisesRegex(ValueError, "nothing to give way"):
            B.give_way_plan(self.baseline, self.seed, B.payload(self.seed, L0), b"\0" + sector[1:])

    def test_restore_refuses_anything_but_a_cleared_first_marker_byte(self):
        length = len(B.payload(self.stock))
        good = B.payload(self.stock)
        for name, current in (("intact", good), ("second byte", good[:1] + b"\0" + good[2:]),
                              ("body", b"\0" + good[1:5000] + bytes([good[5000] ^ 1]) + good[5001:length])):
            with self.subTest(name), self.assertRaisesRegex(ValueError, "only its first marker byte cleared"):
                B.images(self.stock, self.stock, current, restore=True)


if __name__ == "__main__":
    unittest.main()
