"""Build identities bind source snapshots without bootstrapping dependencies."""
import contextlib
import importlib.util
import io
from pathlib import Path
import re
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "firmware/tools"))
SPEC = importlib.util.spec_from_file_location("firmware_build_identity", ROOT / "firmware/tools/build.py")
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)
sys.path.pop(0)


class BuildIdentityTests(unittest.TestCase):
    def test_identity_is_order_independent_and_binds_profile_and_content(self):
        first = {ROOT / "Cargo.toml": "a" * 64, ROOT / "build.rs": "b" * 64}
        identity = BUILD.source_identity("plaf103-firstboot", first)
        self.assertRegex(identity, r"^[0-9a-f]{64}$")
        self.assertEqual(identity, BUILD.source_identity("plaf103-firstboot", dict(reversed(list(first.items())))))
        self.assertNotEqual(identity, BUILD.source_identity("bringup", first))
        self.assertNotEqual(identity, BUILD.source_identity("plaf103-firstboot", first | {ROOT / "build.rs": "c" * 64}))

    def test_offline_identity_command_never_bootstraps(self):
        output = io.StringIO()
        with patch.object(sys, "argv", ["build.py", "source-id"]), \
                patch.object(BUILD, "bootstrap", side_effect=AssertionError("must stay offline")), \
                contextlib.redirect_stdout(output):
            BUILD.main()
        self.assertTrue(re.fullmatch(r"[0-9a-f]{64}\n", output.getvalue()))
