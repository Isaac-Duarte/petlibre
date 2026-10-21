"""Verify source interposition and reject bypasses in real target code/tables."""
import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from support import evidence

ROOT = Path(__file__).resolve().parents[2]


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    sys.modules[name] = value
    spec.loader.exec_module(value)
    return value


rampark = module("rampark", "firmware/tools/rampark.py")
sdkhooks = module("sdkhooks", "firmware/tools/sdkhooks.py")
SOURCE = ROOT / "private/upstream/ameba-rtos-d" / sdkhooks.SOURCE
BUILD = ROOT / "private/firmware-build/plaf103"
LOCK = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
BINUTILS = ROOT / "private/toolchains" / LOCK["toolchain"]["directory"] / "bin"
PROJECT = Path("project/realtek_amebaD_va0_example/GCC-RELEASE")


@evidence(SOURCE.is_file(), "Requires pinned ignored SDK checkout")
class OverlayTests(unittest.TestCase):
    def test_definitions_renamed_and_internal_callers_preserved(self):
        original = SOURCE.read_text()
        changed = sdkhooks.overlay(original)
        for public, legacy in sdkhooks.RENAMES.items():
            self.assertEqual(changed.count(public + "();"), original.count(public + "();"))
            self.assertEqual(changed.count("void " + legacy + "("), 1)
            self.assertNotIn("void " + public + "(", changed)

    def test_missing_or_duplicate_sdk_definition_is_rejected(self):
        original = SOURCE.read_text()
        for name in sdkhooks.RENAMES:
            with self.subTest(name=name), self.assertRaises(ValueError):
                sdkhooks.overlay(original + "\nvoid " + name + "(void) {}\n")
            with self.subTest(name=name), self.assertRaises(ValueError):
                sdkhooks.overlay(original.replace("void " + name + "(", "void unrelated_hook("))


@evidence((BUILD / "manifest.json").is_file() and (BINUTILS / "arm-none-eabi-objdump").is_file(),
                     "Requires completed ignored SDK build and pinned binutils")
class RoutingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not json.loads((BUILD / "manifest.json").read_text()).get("sdk_hook_audit"):
            raise unittest.SkipTest("Requires build with SDK interposer")
        cls.images = []
        for core in ("lp", "hp"):
            elf = BUILD / "sdk" / PROJECT / ("project_" + core) / "asdk/image/target_img2.axf"
            names = sdkhooks.symbols(elf, BINUTILS / "arm-none-eabi-objdump")
            listing = rampark.tool(BINUTILS / "arm-none-eabi-objdump", "-d", "-j", ".ram_image2.text",
                                  "-j", ".xip_image2.text", elf)
            cls.images.append((core, elf, names, listing))

    def test_real_hooks_and_ipc_tables_are_interposed(self):
        result = sdkhooks.audit(BUILD / "sdk", PROJECT, BINUTILS, {"KM0": BUILD / "rust-lib/libpetlibro_km0.a", "KM4": BUILD / "rust-lib/libpetlibro_km4.a"})
        self.assertEqual(set(result["images"]), {"KM0", "KM4"})
        self.assertFalse(result["protocol_installed"])
        self.assertFalse(result["global_exclusion_proven"])

    def test_legacy_entry_bypass_in_target_listing_is_rejected(self):
        for core, _, names, listing in self.images:
            for legacy in sdkhooks.RENAMES.values():
                address = names[legacy]["address"]
                bypass = listing + f"\n00000000 <unmediated_sdk_caller>:\n 0: f000 f800 bl {address:x} <{legacy}>\n"
                with self.subTest(core=core, legacy=legacy), self.assertRaisesRegex(ValueError, "bypasses hook"):
                    sdkhooks.legacy_routes(bypass)

    def test_ipc_table_changed_back_to_legacy_handler_is_rejected(self):
        for core, elf, names, _ in self.images:
            with self.subTest(core=core), tempfile.TemporaryDirectory(dir=BUILD) as directory:
                address = names["ipc_init_config"]["address"] + 28
                offset = sdkhooks.file_offset(elf, BINUTILS / "arm-none-eabi-objdump", address, 4)
                damaged = bytearray(elf.read_bytes())
                struct.pack_into("<I", damaged, offset, names["petlibro_sdk_flash_legacy_ipc"]["address"] | 1)
                path = Path(directory) / "bypassed.axf"
                path.write_bytes(damaged)
                with self.assertRaisesRegex(ValueError, "bypasses the public hook"):
                    sdkhooks.verify_ipc_table(path, BINUTILS / "arm-none-eabi-objdump", names)


if __name__ == "__main__":
    unittest.main()
