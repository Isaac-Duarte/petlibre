"""Flash-write reference audit helpers (firmware/tools/wifiaudit.py); no ELF needed."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "firmware/tools"))
import wifiaudit as W  # noqa: E402


class BranchTests(unittest.TestCase):
    def test_conditional_calls_are_references(self):
        for line in (" e00cb4c:\tf044 fccc \tbleq\te0514e8 <__FLASH_EraseXIP_veneer>",
                     " 1000:\tf000 f800 \tbl\t10005910 <FLASH_EraseXIP>",
                     " 1000:\tf000 b800 \tb.w\t10005910 <FLASH_EraseXIP>",
                     " 1000:\td900      \tbls.n\t10005910 <FLASH_EraseXIP>",
                     " 1000:\t4780      \tblx\t10005910 <FLASH_EraseXIP>"):
            with self.subTest(line):
                match = W.BRANCH.search(line)
                self.assertIsNotNone(match)
                self.assertIn(match[2], ("FLASH_EraseXIP", "__FLASH_EraseXIP_veneer"))
        self.assertIsNone(W.BRANCH.search(" 1000:\t4770 \tbx\tlr"))


class InstructionTests(unittest.TestCase):
    def test_instructions_are_code_and_literal_words_are_not(self):
        # This eor.w's encoding equals shell_cmd_table's address in one build.
        insn = W.INSTRUCTION.match(" e014a28:\tea80 0e06 \teor.w\tlr, r0, r6")
        self.assertEqual((insn[1], insn[2].replace(" ", ""), insn[3]), ("e014a28", "ea800e06", "eor.w"))
        self.assertEqual(W.INSTRUCTION.match(" e014a2c:\t989e      \tldr\tr0, [sp, #632]")[3], "ldr")
        word = W.INSTRUCTION.match("100057bc:\t100210f8 \t.word\t0x100210f8")
        self.assertTrue(word[3].startswith("."))


class DemangleTests(unittest.TestCase):
    def test_rust_paths_drop_the_crate_hash(self):
        self.assertEqual(W.demangle("_RNvNtCseBw8xjNrJWj_19wellfed_amebad_sdk6target13bank1_program"),
                         "wellfed_amebad_sdk::target::bank1_program")
        self.assertEqual(W.demangle("_RNvNtCsOther123_19wellfed_amebad_sdk6target11bank1_erase"),
                         "wellfed_amebad_sdk::target::bank1_erase")

    def test_other_names_are_unchanged(self):
        for name in ("FLASH_EraseXIP", "cmd_flash", "_RNvMs_NtCsx_5crate3modNtB4_4Type3new", "_Rgarbage"):
            with self.subTest(name):
                self.assertEqual(W.demangle(name), name)


if __name__ == "__main__":
    unittest.main()
