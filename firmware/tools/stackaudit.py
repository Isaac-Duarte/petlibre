"""Worst-case static stack of each KM4 application task, from the linked image.

A crate-split build overflowed the optics task's 4 KiB stack on hardware: LTO
inlined a 3 KiB temporary into the task's frame. The SDK's ARMv8-M port faults
on the stack limit, and FW11's trial caught it, but only after an update. This
audit catches it at build time instead.

For each task created in `app.rs` (`sdk::task_create(c"name", words, ...)`),
it walks the direct call graph from the task's entry function and sums the
frames along the deepest path. Frames come from each function's prologue
(`push`, `stmdb sp!`, `vpush`, `sub sp`). Recursion is cut at the cycle, and
indirect calls (`blx rN`, function pointers, `dyn` formatting) cannot be
followed, so the bound requires a margin rather than an exact fit.
"""
import re

import rampark
import wifiaudit

# The SDK trampoline frame and an exception stacked onto the task's stack
# with the FPU context (26 words), rounded up.
OVERHEAD = 256
# Unfollowed indirect calls: the bound must leave this share of the stack free.
MARGIN = 0.25

TASK = re.compile(r'task_create\(\s*c"([\w-]+)",\s*(\w+),\s*\d+,\s*(\w+),', re.S)
CONSTANT = re.compile(r"const (\w+): u16 = (\d+);")
HEADER = re.compile(r"^([0-9a-f]+) <([^>]+)>:$")
CALL = re.compile(r"\t(?:bl|blx|b\.w|b)(?:\.[nw])?\t[0-9a-f]+ <([^>+]+)>")
INDIRECT = re.compile(r"\tblx\tr\d+\b")


def tasks(source):
    """{entry function name: (task name, stack bytes)} from `app.rs` text."""
    constants = {name: int(value) for name, value in CONSTANT.findall(source)}
    found = {}
    for name, words, entry in TASK.findall(source):
        words = constants[words] if words in constants else int(words)
        found[entry] = (name, words * 4)
    return found


def frames(listing):
    """{function: (frame bytes, direct callees, has indirect calls)} from `objdump -d`."""
    functions, current = {}, None
    for line in listing.splitlines():
        header = HEADER.match(line)
        if header:
            current = header[2]
            functions[current] = {"frame": 0, "calls": set(), "indirect": False, "sized": False}
            continue
        if current is None or ":\t" not in line:
            continue
        f = functions[current]
        insn = line.split("\t", 2)[-1]
        if not f["sized"]:
            push = re.match(r"(?:push(?:\.w)?|stmdb(?:\.w)?\s+sp!,)\s*\{([^}]*)\}", insn)
            if push:
                f["frame"] += 4 * len([r for r in push[1].split(",") if r.strip()])
                continue
            vpush = re.match(r"vpush\s*\{d(\d+)-d(\d+)\}", insn)
            if vpush:
                f["frame"] += 8 * (int(vpush[2]) - int(vpush[1]) + 1)
                continue
            sub = re.match(r"sub(?:\.w)?\s+sp,\s*(?:sp,\s*)?#(\d+)", insn)
            if sub:
                f["frame"] += int(sub[1])
                f["sized"] = True
                continue
        call = CALL.search(line)
        if call:
            f["calls"].add(call[1])
        elif INDIRECT.search(line):
            f["indirect"] = True
    return functions


def worst(functions, entry):
    """(bytes, deepest path, indirect calls on any path, recursion cut)."""
    memo, cut = {}, set()

    def walk(name, visiting):
        if name in visiting:
            cut.add(name)
            return 0, [], False
        if name in memo:
            return memo[name]
        f = functions.get(name)
        if f is None:
            return 0, [name], False
        best, path, indirect = 0, [], f["indirect"]
        for callee in sorted(f["calls"]):
            depth, deeper, nested = walk(callee, visiting | {name})
            indirect = indirect or nested
            if depth > best:
                best, path = depth, deeper
        memo[name] = (f["frame"] + best, [name] + path, indirect)
        return memo[name]

    depth, path, indirect = walk(entry, frozenset())
    return depth, path, indirect, sorted(cut)


def audit(elf, objdump, source):
    """Per-task bounds; raises if a task's bound leaves less than MARGIN free."""
    functions = frames(rampark.tool(objdump, "-d", elf))
    by_short = {}
    for name in functions:
        by_short.setdefault(wifiaudit.demangle(name).rsplit("::", 1)[-1], []).append(name)

    rows = {}
    for entry, (task, stack) in sorted(tasks(source).items()):
        symbols = by_short.get(entry, [])
        if not symbols:
            continue  # Not linked in this variant.
        if len(symbols) != 1:
            raise ValueError(f"Ambiguous task entry {entry}: {symbols}")
        depth, path, indirect, cut = worst(functions, symbols[0])
        bound = depth + OVERHEAD
        rows[task] = {"entry": entry, "stack": stack, "bound": bound, "indirect": indirect,
                      "recursion": [wifiaudit.demangle(n) for n in cut],
                      "path": [wifiaudit.demangle(n) for n in path[:12]]}
        if bound > stack * (1 - MARGIN):
            raise ValueError(f"Task {task} may overflow: static bound {bound} bytes of {stack} "
                             f"(margin {MARGIN:.0%}); deepest path {rows[task]['path'][:6]}")
    if not rows:
        raise ValueError("No application task entries found in the linked image")
    return {"tasks": rows, "overhead": OVERHEAD, "margin": MARGIN,
            "scope": "Direct calls and prologue frames only; indirect calls are covered by the margin, "
                     "not bounded"}
