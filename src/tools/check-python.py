#!/usr/bin/env python3
"""Run contributor tests or explicitly selected binary-evidence checks."""
import argparse
import importlib.metadata
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", action="store_true",
                        help="run optional evidence tests; missing fixtures fail")
    args = parser.parse_args()
    for line in (ROOT / "src/tools/requirements-probe.txt").read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        name, expected = line.strip().split("==")
        try:
            installed = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            installed = "missing"
        if installed != expected:
            print(f"Requires {name}=={expected}; found {installed}. "
                  "Install src/tools/requirements-probe.txt into an ignored environment "
                  "and select it with PYTHON. See docs/development/README.md.", file=sys.stderr)
            return 2

    discovered = unittest.TestLoader().discover(str(ROOT / "src/tools/tests"))
    selected, excluded = [], 0
    for case in cases(discovered):
        method = getattr(case, case._testMethodName, None)
        optional = bool(getattr(type(case), "__wellfed_evidence__", False)
                        or getattr(method, "__wellfed_evidence__", False))
        # Import errors must fail either suite, never disappear through filtering.
        failed_import = isinstance(case, unittest.loader._FailedTest)
        if failed_import or optional == args.fixtures:
            selected.append(case)
        else:
            excluded += 1
    if not selected:
        print("No Python tests discovered for the selected suite; refusing an empty check.",
              file=sys.stderr)
        return 1
    print(f"Selected {len(selected)} tests; {excluded} tests belong to the other suite.",
          file=sys.stderr)
    result = unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(selected))
    if result.skipped:
        print(f"Selected suite requires no skips; {len(result.skipped)} checks unavailable. "
              "See docs/development/fixtures.md.", file=sys.stderr)
        return 1
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
