"""First-boot artifact checks using the actual linked UART-only SDK images."""
import importlib.util
import json
from pathlib import Path
import shutil
import struct
import sys
import tempfile
import unittest
from support import current_build, evidence

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "firmware/tools"))
import firstboot
sys.path.pop(0)
spec = importlib.util.spec_from_file_location("firstboot_preflight", ROOT / "tools/flash/prepare-firstboot.py")
preflight = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preflight)
BUILD = ROOT / "private/firmware-build/plaf103-firstboot"
PROJECT = Path("project/realtek_amebaD_va0_example/GCC-RELEASE")
LOCK = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
BINUTILS = ROOT / "private/toolchains" / LOCK["toolchain"]["directory"] / "bin"
CURRENT = ROOT / "private/fixtures/current-read/flash-read.bin"
FRESH, STALE = current_build(BUILD)


@evidence(FRESH and CURRENT.is_file(),
          STALE or "Requires UART-only first-boot build and fresh ignored backup")
class FirstBootTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = json.loads((BUILD / "manifest.json").read_text())
        cls.listings = []
        for core, function in cls.manifest["firstboot_startup_audit"]["mains"].items():
            elf = BUILD / "sdk" / PROJECT / f"project_{core}/asdk/image/target_img2.axf"
            symbols = {}
            for line in firstboot.rampark.tool(BINUTILS / "arm-none-eabi-objdump", "-t", elf).splitlines():
                fields = line.split()
                if len(fields) >= 4:
                    try:
                        symbols[fields[-1]] = int(fields[0], 16)
                    except ValueError:
                        pass
            address, size = int(function["address"], 16), function["bytes"]
            listing = firstboot.rampark.tool(BINUTILS / "arm-none-eabi-objdump", "-d",
                "--start-address=" + hex(address), "--stop-address=" + hex(address + size), elf)
            veneers = {symbols["__" + n + "_veneer"]: n for n in firstboot.EXPECTED[core]
                       if "__" + n + "_veneer" in symbols}
            cls.listings.append((core, address, size, listing, symbols, veneers))
        artifact = next(a for a in cls.manifest["artifacts"] if Path(a["path"]).name == "OTA_All.bin")
        cls.image = (BUILD / artifact["path"]).read_bytes()

    def test_actual_mains_resolve_only_expected_calls(self):
        result = firstboot.audit(BUILD / "sdk", PROJECT, BINUTILS)
        self.assertFalse(result["network_or_shell_started_by_main"])
        for core, main in result["mains"].items():
            self.assertEqual(tuple(main["resolved_calls"]), firstboot.EXPECTED[core])
            self.assertEqual(main["unresolved_calls"], 0)

    def test_unknown_calls_control_flow_and_mmio_stores_are_rejected(self):
        for core, address, size, listing, symbols, veneers in self.listings:
            for instruction in ("f000 f800 bl 12345678 <wlan_network>", "4798 blx r7",
                                "4700 bx r0", "4687 mov pc, r0", "6001 str r1, [r0]",
                                "e7fe b.n %x <main>" % address):
                with self.subTest(core=core, instruction=instruction):
                    damaged = f" {address:x}: {instruction}\n" + listing
                    with self.assertRaises(ValueError):
                        firstboot.check_main(damaged, address, size, core, symbols, veneers)

    def test_changed_rom_pointer_is_rejected(self):
        for core, address, size, listing, symbols, veneers in self.listings:
            value = symbols["irq_register"]
            changed = listing.replace(f".word\t0x{value:08x}", ".word\t0x12345679")
            self.assertNotEqual(changed, listing)
            with self.assertRaises(ValueError):
                firstboot.check_main(changed, address, size, core, symbols, veneers)

    def test_veneer_target_corruption_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            sdk = Path(temporary)
            for core in ("lp", "hp"):
                relative = PROJECT / f"project_{core}/asdk/image/target_img2.axf"
                target = sdk / relative
                target.parent.mkdir(parents=True)
                shutil.copyfile(BUILD / "sdk" / relative, target)
                if core == "hp":
                    symbols = next(row[4] for row in self.listings if row[0] == core)
                    address = symbols["__vTaskStartScheduler_veneer"]
                    at = firstboot.sdkhooks.file_offset(target, BINUTILS / "arm-none-eabi-objdump", address, 16)
                    data = bytearray(target.read_bytes())
                    struct.pack_into("<I", data, at + 12, (symbols["vTaskStartScheduler"] | 1) ^ 4)
                    target.write_bytes(data)
            with self.assertRaisesRegex(ValueError, "veneer"):
                firstboot.audit(sdk, PROJECT, BINUTILS)

    def test_default_sdk_mains_are_refused(self):
        with self.assertRaises(ValueError):
            firstboot.audit(ROOT / "private/firmware-build/plaf103/sdk", PROJECT, BINUTILS)

    def test_actual_factory_accepts_short_diagnostic_in_both_banks(self):
        cases = preflight.mapping_cases(CURRENT.read_bytes(), preflight.FACTORY.ROM.read_bytes(), self.image)
        self.assertEqual([case["selection"]["result"] for case in cases], [1, 2])
        self.assertLess(len(self.image), 200000)

    def test_stock_guard_and_hypothetical_restoration_preserve_fresh_snapshot(self):
        baseline, current = preflight.FACTORY.FLASH.read_bytes(), CURRENT.read_bytes()
        report = preflight.restoration_fixture(baseline, current, self.image)
        self.assertTrue(report["live_custom_bank_refused_by_existing_cli"])
        self.assertTrue(report["invalidation_is_model_only"])
        self.assertTrue(report["restores_exact_current_snapshot"])
        self.assertEqual(report["stock_restoration_plan"]["proposed_snapshot_sha256"], preflight.digest(current))


if __name__ == "__main__":
    unittest.main()
