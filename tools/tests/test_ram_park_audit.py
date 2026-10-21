"""Use both real target listings to exercise the RAM leaf verifier and rejection."""
import importlib.util
import json
from pathlib import Path
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("rampark", ROOT / "firmware/tools/rampark.py")
rampark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rampark)
BUILD = ROOT / "private/firmware-build/plaf103"
MANIFEST = BUILD / "manifest.json"
LOCK = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
BINUTILS = ROOT / "private/toolchains" / LOCK["toolchain"]["directory"] / "bin"


@evidence(MANIFEST.is_file() and (BINUTILS / "arm-none-eabi-objdump").is_file(),
                     "Requires completed ignored SDK build and pinned binutils")
class RamParkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        manifest = json.loads(MANIFEST.read_text())
        if not manifest.get("ram_park_audit"):
            raise unittest.SkipTest("Requires model build with retained RAM parking routines")
        cls.listings = []
        for core, image in manifest["ram_park_audit"]["images"].items():
            project = "project_lp" if core == "KM0" else "project_hp"
            elf = BUILD / "sdk/project/realtek_amebaD_va0_example/GCC-RELEASE" / project / "asdk/image/target_img2.axf"
            for name, function in image["functions"].items():
                address, size = int(function["address"], 16), function["bytes"]
                listing = rampark.tool(BINUTILS / "arm-none-eabi-objdump", "-d",
                    "--start-address=" + hex(address), "--stop-address=" + hex(address + size), elf)
                cls.listings.append((core, name, address, size, listing))

    def test_retained_functions_in_both_packaged_cores(self):
        result = rampark.audit(BUILD / "sdk", Path("project/realtek_amebaD_va0_example/GCC-RELEASE"), BINUTILS, {"KM0": BUILD / "rust-lib/libpetlibro_km0.a", "KM4": BUILD / "rust-lib/libpetlibro_km4.a"})
        self.assertEqual(set(result["images"]), {"KM0", "KM4"})
        self.assertFalse(result["activated"])
        for image in result["images"].values():
            self.assertEqual(set(image["functions"]), set(rampark.SYMBOLS))
            self.assertTrue(all(f["calls"] == 0 for f in image["functions"].values()))

    def test_calls_and_foreign_branches_are_rejected(self):
        for core, name, address, size, listing in self.listings:
            for instruction in ("f000 f800 bl 1010b478 <ROM_call>",
                                "e000 b.n 0e000020 <XIP_target>", "4700 bx r0",
                                "e8b0 8000 ldmia.w r0!, {pc}", "4687 mov pc, r0",
                                f"f8df f000 ldr.w pc, [pc, #0] ; ({address:x} <bad_jump>)"):
                with self.subTest(core=core, name=name, instruction=instruction):
                    damaged = listing + f"\n {address:x}: {instruction}\n"
                    with self.assertRaises(ValueError):
                        rampark.check_leaf(damaged, address, size, 1 if "_sem_" in name else 3)

    def test_foreign_literals_and_unreviewed_instructions_are_rejected(self):
        for core, name, address, size, listing in self.listings:
            for instruction in ("0e000020 .word 0x0e000020",
                                "4800 ldr r0, [pc, #0] ; (0e000020 <bad_literal>)",
                                "ed90 0a00 vldr s0, [r0]", "f000 ????"):
                with self.subTest(core=core, name=name, instruction=instruction):
                    damaged = listing + f"\n {address:x}: {instruction}\n"
                    with self.assertRaises(ValueError):
                        rampark.check_leaf(damaged, address, size, 1 if "_sem_" in name else 3)


if __name__ == "__main__":
    unittest.main()
