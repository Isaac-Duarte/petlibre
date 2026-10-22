"""Check ELF/package pairing against real compiled code, including stale ELFs."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from support import current_build, evidence

ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


users = module("sdk_flash_users", "tools/audit/audit-sdk-flash-users.py")
layout = module("ota_layout_for_flash_users", "tools/audit/audit-ota-layout.py")
BUILD = ROOT / "private/firmware-build/plaf103"
FRESH, STALE = current_build(BUILD)
ELFS = {core: BUILD / users.PROJECT / project / "asdk/image/target_img2.axf"
        for core, project in (("KM0", "project_lp"), ("KM4", "project_hp"))}


@evidence(FRESH and
                     all(path.is_file() for path in ELFS.values()) and
                     (users.DEFAULT_TOOLS / "arm-none-eabi-objdump").is_file(),
                     STALE or "Requires ignored completed SDK build and pinned binutils")
class PackagedCodeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        manifest = json.loads((BUILD / "manifest.json").read_text())
        ota = next(a for a in manifest["artifacts"] if Path(a["path"]).name == "OTA_All.bin")
        cls.payload = layout.container((BUILD / ota["path"]).read_bytes())
        cls.regions = layout.regions(cls.payload)
        cls.objdump = users.DEFAULT_TOOLS / "arm-none-eabi-objdump"

    def test_matches_packaged_text_for_each_core(self):
        for core, elf in ELFS.items():
            with self.subTest(core=core):
                checked = users.verify_packaged_code(elf, self.objdump, self.payload, self.regions, core)
                self.assertEqual({x["section"] for x in checked},
                                 {".ram_image2.text", ".xip_image2.text"})
                self.assertTrue(all(x["bytes"] > 0 for x in checked))

    def test_changed_code_is_rejected_with_unchanged_packaged_image(self):
        for core, elf in ELFS.items():
            original = elf.read_bytes()
            for kind in ("ram", "xip"):
                with self.subTest(core=core, kind=kind), tempfile.TemporaryDirectory(dir=BUILD) as directory:
                    region = next(r for r in self.regions if r["name"] == core.lower() + "_" + kind)
                    # Locate unique compiled instructions from the independent
                    # package, rather than copying the audit's section offsets.
                    start = region["payload_header_offset"] + 32 + (32 if kind == "ram" else 0)
                    instructions = self.payload[start:start + 64]
                    self.assertEqual(original.count(instructions), 1)
                    damaged = bytearray(original)
                    damaged[original.index(instructions) + 8] ^= 1
                    changed = Path(directory) / "stale.axf"
                    changed.write_bytes(damaged)
                    with self.assertRaisesRegex(ValueError, "does not match packaged image"):
                        users.verify_packaged_code(changed, self.objdump, self.payload, self.regions, core)


if __name__ == "__main__":
    unittest.main()
