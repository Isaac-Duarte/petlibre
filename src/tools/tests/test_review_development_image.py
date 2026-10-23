"""Frozen development-image reviews: refusals before anything is written."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("review_development_test", ROOT / "src/tools/flash/review-development-image.py")
REVIEW = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REVIEW)


class ReviewTests(unittest.TestCase):
    def test_unknown_or_stale_build_refused_before_freezing(self):
        (ROOT / "private").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / "private") as directory:
            root = Path(directory)
            build = root / "build"
            build.mkdir()
            (build / "manifest.json").write_text(json.dumps({"profile": "bringup"}))
            with self.assertRaisesRegex(ValueError, "startup profile"):
                REVIEW.freeze("build", build, root / "output")
            self.assertFalse((root / "output").exists())

    def test_payload_must_fit_the_aligned_bank(self):
        with self.assertRaisesRegex(ValueError, "Immutable complete OTA bytes"):
            REVIEW.bank_payload(bytearray(b"OTA"))
        with self.assertRaises(ValueError):
            REVIEW.bank_payload(b"\0" * 64)


if __name__ == "__main__":
    unittest.main()
