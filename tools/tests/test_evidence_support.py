"""Optional evidence suites skip, naming the changed inputs, when a build is stale."""
import json
from pathlib import Path
import tempfile
import unittest

from support import current_build, current_inputs


class EvidenceFreshnessTests(unittest.TestCase):
    def test_stale_or_missing_build_manifests_are_unavailable(self):
        with tempfile.TemporaryDirectory() as temporary:
            build = Path(temporary) / "baseline"
            build.mkdir()
            self.assertFalse(current_build(build)[0])
            inputs = current_inputs("baseline")
            self.assertIn("firmware/sdk.lock.json", inputs)
            (build / "manifest.json").write_text(json.dumps({"profile": "baseline", "inputs": inputs}))
            self.assertEqual(current_build(build), (True, ""))
            changed = dict(inputs, **{"firmware/sdk.lock.json": "0" * 64})
            (build / "manifest.json").write_text(json.dumps({"profile": "baseline", "inputs": changed}))
            available, reason = current_build(build)
            self.assertFalse(available)
            self.assertIn("Stale baseline build", reason)
            self.assertIn("firmware/sdk.lock.json", reason)


if __name__ == "__main__":
    unittest.main()
