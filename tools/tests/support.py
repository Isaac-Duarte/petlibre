"""Mark optional evidence checks separately from contributor checks."""
import importlib.util
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


def evidence(available, reason):
    """Select only for --fixtures, failing that suite if unavailable."""
    def decorate(value):
        value.__wellfed_evidence__ = True
        return unittest.skipUnless(available, reason)(value)
    return decorate


def _builder():
    sys.path.insert(0, str(ROOT / "firmware/tools"))
    try:
        spec = importlib.util.spec_from_file_location("evidence_build_inputs", ROOT / "firmware/tools/build.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    finally:
        sys.path.remove(str(ROOT / "firmware/tools"))
    return module


def current_build(directory):
    """Return (available, reason) for an ignored SDK build manifest.

    A manifest whose recorded inputs differ from the current source tree is
    stale evidence: the suite is unavailable (skipped), not passed or failed.
    """
    manifest = Path(directory) / "manifest.json"
    if not manifest.is_file():
        return False, f"Requires completed ignored SDK build: {manifest.parent.name}"
    try:
        recorded = json.loads(manifest.read_text())
        builder = _builder()
        current = {str(p.relative_to(ROOT)): builder.digest(p)
                   for p in builder.source_inputs(recorded["profile"])}
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        return False, f"Cannot compare {manifest} with current sources: {error}"
    if recorded.get("inputs") != current:
        changed = sorted(set(current) ^ set(recorded.get("inputs", {})) |
                         {k for k in current if recorded.get("inputs", {}).get(k) not in (None, current[k])})
        return False, (f"Stale {manifest.parent.name} build; rebuild it before using it as evidence "
                       f"(changed inputs: {', '.join(changed[:5])}{' ...' if len(changed) > 5 else ''})")
    return True, ""
