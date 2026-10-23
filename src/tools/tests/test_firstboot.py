"""The actual factory loader accepts the first-boot build in either bank (emulated)."""
import importlib.util
import json
from pathlib import Path
import unittest
from support import current_build, evidence

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("firstboot_review", ROOT / "src/tools/flash/review-development-image.py")
review = importlib.util.module_from_spec(spec)
spec.loader.exec_module(review)
BUILD = ROOT / "private/firmware-build/plaf103-firstboot"
CURRENT = ROOT / "private/fixtures/current-read/flash-read.bin"
FRESH, STALE = current_build(BUILD)


@evidence(FRESH and CURRENT.is_file(),
          STALE or "Requires UART-only first-boot build and fresh ignored backup")
class FirstBootTests(unittest.TestCase):
    def test_actual_factory_accepts_diagnostic_in_both_banks(self):
        manifest = json.loads((BUILD / "manifest.json").read_text())
        artifact = next(a for a in manifest["artifacts"] if Path(a["path"]).name == "OTA_All.bin")
        image = (BUILD / artifact["path"]).read_bytes()
        factory = review.factory()
        cases = review.mapping_cases(CURRENT.read_bytes(), factory.ROM.read_bytes(), image)
        self.assertEqual([case["selection"]["result"] for case in cases], [1, 2])
        self.assertLessEqual(len(review.bank_payload(image)), review.BANK_BYTES)


if __name__ == "__main__":
    unittest.main()
