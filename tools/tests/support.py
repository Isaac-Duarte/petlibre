"""Mark optional evidence checks separately from contributor checks."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


def evidence(available, reason):
    """Select only for --fixtures, failing that suite if unavailable."""
    def decorate(value):
        value.__wellfed_evidence__ = True
        return unittest.skipUnless(available, reason)(value)
    return decorate


def current_inputs(profile, firmware=None):
    """A profile's current input digests, from `wellfed-build inputs`, for the
    firmware a build manifest names (the builder's default when it names none)."""
    selected = ["--firmware", firmware] if firmware else []
    try:
        result = subprocess.run(["cargo", "run", "--quiet", "--locked", "--release", "-p", "wellfed-build",
                                 "--", "inputs", profile, *selected], cwd=ROOT, check=True, capture_output=True,
                                text=True)
    except subprocess.CalledProcessError as error:
        raise RuntimeError(error.stderr.strip() or "wellfed-build inputs failed") from None
    return json.loads(result.stdout)


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
        current = current_inputs(recorded["profile"], recorded.get("firmware"))
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        return False, f"Cannot compare {manifest} with current sources: {error}"
    if recorded.get("inputs") != current:
        changed = sorted(set(current) ^ set(recorded.get("inputs", {})) |
                         {k for k in current if recorded.get("inputs", {}).get(k) not in (None, current[k])})
        return False, (f"Stale {manifest.parent.name} build; rebuild it before using it as evidence "
                       f"(changed inputs: {', '.join(changed[:5])}{' ...' if len(changed) > 5 else ''})")
    return True, ""
