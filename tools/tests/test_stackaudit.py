"""Static task-stack audit helpers (firmware/tools/stackaudit.py); no ELF needed."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "firmware/tools"))
import stackaudit as S  # noqa: E402

LISTING = """\
00001000 <task>:
    1000:\tb5f0      \tpush\t{r4, r5, r6, r7, lr}
    1002:\tf5ad 6d4d \tsub.w\tsp, sp, #3280\t; 0xcd0
    1006:\tf000 f800 \tbl\t2000 <deep>
    100a:\tf000 f800 \tbl\t3000 <shallow>
    100e:\t4798      \tblx\tr3

00002000 <deep>:
    2000:\te92d 4ff0 \tstmdb\tsp!, {r4, r5, r6, r7, r8, r9, sl, fp, lr}
    2004:\ted2d 8b04 \tvpush\t{d8-d9}
    2008:\tb0d1      \tsub\tsp, #836
    200a:\tf000 f800 \tbl\t2000 <deep>
    200e:\tf000 b800 \tb.w\t3000 <shallow>

00003000 <shallow>:
    3000:\tb510      \tpush\t{r4, lr}
    3002:\tbd10      \tpop\t{r4, pc}
"""


class StackTests(unittest.TestCase):
    def test_tasks_resolve_word_constants(self):
        source = 'const PROBE_STACK_WORDS: u16 = 2048;\n' \
                 'sdk::task_create(\n    c"wellfed-optics",\n    PROBE_STACK_WORDS,\n    3,\n    optics_task,\n' \
                 'let ok = sdk::task_create(c"wellfed-wdg", 512, 3, feeder_task, ptr::null_mut());'
        self.assertEqual(S.tasks(source), {"optics_task": ("wellfed-optics", 8192),
                                           "feeder_task": ("wellfed-wdg", 2048)})

    def test_deepest_path_sums_prologues_and_cuts_recursion(self):
        functions = S.frames(LISTING)
        self.assertEqual(functions["task"]["frame"], 20 + 3280)
        self.assertTrue(functions["task"]["indirect"])
        self.assertEqual(functions["deep"]["frame"], 36 + 16 + 836)
        depth, path, indirect, cut = S.worst(functions, "task")
        self.assertEqual(depth, 3300 + 888 + 8)
        self.assertEqual(path, ["task", "deep", "shallow"])
        self.assertTrue(indirect)
        self.assertEqual(cut, ["deep"])


if __name__ == "__main__":
    unittest.main()
