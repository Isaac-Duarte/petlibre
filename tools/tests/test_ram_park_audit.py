"""Use both real target listings to exercise the RAM leaf verifier and rejection."""
import importlib.util
import json
from pathlib import Path
import unittest
from support import current_build, evidence

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("rampark", ROOT / "firmware/tools/rampark.py")
rampark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rampark)
BUILD = ROOT / "private/firmware-build/plaf103"
MANIFEST = BUILD / "manifest.json"
LOCK = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
BINUTILS = ROOT / "private/toolchains" / LOCK["toolchain"]["directory"] / "bin"
FRESH, STALE = current_build(BUILD)


@evidence(FRESH and (BINUTILS / "arm-none-eabi-objdump").is_file(),
          STALE or "Requires completed ignored SDK build and pinned binutils")
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


class FlashAddressTests(unittest.TestCase):
    """Synthetic listings in the pinned objdump format; no SDK build required."""
    ADDRESS = 0x10005000

    def listing(self, *body):
        lines = [*body, "f3bf 8f4f dsb sy", "f3bf 8f4f dsb sy", "f3bf 8f4f dsb sy", "4770 bx lr"]
        return "".join(f"{self.ADDRESS + 4 * i:x}:\t{line}\n" for i, line in enumerate(lines))

    def check(self, *body):
        return rampark.check_leaf(self.listing(*body), self.ADDRESS, 4 * (len(body) + 4))

    def test_mmio_and_ram_constants_are_accepted(self):
        result = self.check("f242 0014 movw r0, #8212 ; 0x2014", "f6c4 0000 movt r0, #18432 ; 0x4800",
                            "f241 0100 movw r1, #4096 ; 0x1000", "f2c1 0100 movt r1, #4096 ; 0x1000",
                            "f04f 6300 mov.w r3, #134217728 ; 0x8000000")
        self.assertEqual(result["flash_address_constants"], 0)
        self.assertEqual(result["barriers"], 3)

    def test_flash_literal_words_are_rejected_across_the_whole_mapping(self):
        for value in (0x08000000, 0x08186000, 0x0BFFFFFF, 0x0C000020, 0x0E000020, 0x0FFFFFFF):
            with self.subTest(value=hex(value)), self.assertRaisesRegex(ValueError, "Flash/XIP literal"):
                self.check(f"{value:08x} .word 0x{value:08x}")
        for value in (0x07FFFFFF, 0x10000000, 0x48002014):
            with self.subTest(value=hex(value)):
                self.check(f"{value:08x} .word 0x{value:08x}")

    def test_movw_movt_flash_addresses_are_rejected(self):
        for body in (("f246 0000 movw r0, #24576 ; 0x6000", "f6c0 0000 movt r0, #2048 ; 0x800"),
                     ("f6c0 6000 movt r0, #3584 ; 0xe00",),
                     ("f240 0320 movw r3, #32 ; 0x20", "2200 movs r2, #0",
                      "f6c0 4300 movt r3, #3072 ; 0xc00"),
                     ("f6c0 0300 movteq r3, #2048 ; 0x800",)):
            with self.subTest(body=body), self.assertRaisesRegex(ValueError, "movw/movt"):
                self.check(*body)

    def test_unrelated_register_write_discards_low_half(self):
        # r0's low half is overwritten before movt; the high half alone is RAM.
        self.check("f246 0000 movw r0, #24576 ; 0x6000", "2000 movs r0, #0",
                   "f2c1 0000 movt r0, #4096 ; 0x1000")


if __name__ == "__main__":
    unittest.main()
