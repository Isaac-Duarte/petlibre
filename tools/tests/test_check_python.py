"""Check command exits distinguish skips, missing setup and actual failures."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class CheckCommandTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "tools/tests").mkdir(parents=True)
        for filename in ("check-python.py", "requirements-probe.txt"):
            shutil.copyfile(ROOT / "tools" / filename, self.root / "tools" / filename)
        shutil.copyfile(ROOT / "tools/tests/support.py", self.root / "tools/tests/support.py")

    def run_check(self, source=None, *args):
        if source is not None:
            (self.root / "tools/tests/test_fixture.py").write_text(source)
        return subprocess.run([sys.executable, "-B", str(self.root / "tools/check-python.py"), *args],
                              capture_output=True, text=True, cwd=self.root)

    def test_synthetic_checks_run_without_private_directory(self):
        result = self.run_check("import unittest\nclass Tests(unittest.TestCase):\n"
                                "    def test_synthetic(self): self.assertEqual(2 + 2, 4)\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Ran 1 test", result.stderr)
        self.assertFalse((self.root / "private").exists())

    def test_missing_evidence_is_excluded_normally_and_fails_when_requested(self):
        source = ("import unittest\nfrom support import evidence\n"
                  "class Tests(unittest.TestCase):\n"
                  "    def test_synthetic(self): pass\n"
                  "    @evidence(False, 'Missing reference binary')\n"
                  "    def test_evidence(self): pass\n")
        ordinary = self.run_check(source)
        strict = self.run_check(source, "--fixtures")
        self.assertEqual(ordinary.returncode, 0, ordinary.stderr)
        self.assertIn("Selected 1 tests; 1 tests belong", ordinary.stderr)
        self.assertNotIn("skipped", ordinary.stderr)
        self.assertEqual(strict.returncode, 1, strict.stderr)
        self.assertIn("Missing reference binary", strict.stderr)
        self.assertIn("requires no skips", strict.stderr)

    def test_present_evidence_remains_separate_from_contributor_checks(self):
        source = ("import unittest\nfrom support import evidence\n"
                  "class Ordinary(unittest.TestCase):\n"
                  "    def test_synthetic(self): pass\n"
                  "@evidence(True, 'available')\n"
                  "class Optional(unittest.TestCase):\n"
                  "    def test_evidence(self): self.fail('fixture sentinel')\n")
        self.assertEqual(self.run_check(source).returncode, 0)
        result = self.run_check(source, "--fixtures")
        self.assertEqual(result.returncode, 1)
        self.assertIn("fixture sentinel", result.stderr)

    def test_unexpected_skips_fail(self):
        result = self.run_check("import unittest\nclass Tests(unittest.TestCase):\n"
                                "    def test_missing(self): self.skipTest('missing input')\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("requires no skips", result.stderr)

    def test_assertion_and_discovery_errors_fail(self):
        for source in ("import unittest\nclass Tests(unittest.TestCase):\n"
                       "    def test_failure(self): self.fail('sentinel failure')\n",
                       "raise RuntimeError('sentinel discovery error')\n"):
            with self.subTest(source=source):
                result = self.run_check(source)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("FAILED", result.stderr)

    def test_missing_or_wrong_dependency_fails_before_discovery(self):
        for requirement in ("petlibro-missing-test-dependency==0\n", "unicorn==0\n"):
            with self.subTest(requirement=requirement):
                (self.root / "tools/requirements-probe.txt").write_text(requirement)
                result = self.run_check("raise RuntimeError('must not discover')\n")
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("Install tools/requirements-probe.txt", result.stderr)
                self.assertNotIn("must not discover", result.stderr)

    def test_empty_discovery_is_not_a_successful_check(self):
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("No Python tests discovered", result.stderr)


if __name__ == "__main__":
    unittest.main()
