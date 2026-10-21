#!/usr/bin/env python3
"""Check local documentation links, tool dependencies, and portable repository inputs.

This is a maintenance check, not a secret scanner or a licensing audit.
"""
import ast
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]


def main():
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=ROOT,
    ).decode().split("\0")
    paths = [ROOT / name for name in names if name]
    errors = []
    links = 0
    for path in paths:
        relative = path.relative_to(ROOT)
        if relative.parts[0] in {"private", ".venv", "target", ".aws", ".codex", ".agents"}:
            errors.append(f"{relative}: local inputs or agent state must not be published")
            continue
        if path.suffix.lower() in {".bin", ".pcap", ".pcapng", ".img", ".apk"}:
            errors.append(f"{relative}: binary/evidence files need explicit provenance review")
        if path.suffix not in {".md", ".rs", ".py", ".toml", ".json", ".yml", ".txt"}:
            continue
        content = path.read_text()
        if re.search(r"/(?:home|Users|mnt|media)/[A-Za-z0-9_.-]+/", content):
            errors.append(f"{relative}: user-specific absolute path")
        if re.search(r"private/[^\s\"']*202610\d{2}", content):
            errors.append(f"{relative}: dated private fixture path")
        if path.suffix == ".md":
            # Inline links only; external references are provenance links, not fetched.
            for match in re.finditer(r"\]\(([^)]+)\)", content):
                target = match.group(1).strip().strip("<>")
                url = urlsplit(target)
                if url.scheme or url.netloc or not url.path:
                    continue
                dest = (path.parent / unquote(url.path)).resolve()
                links += 1
                if not dest.is_relative_to(ROOT) or not dest.exists():
                    errors.append(f"{relative}: missing/nonlocal link {target}")
    # Curated Python entrypoints load siblings dynamically; catch missing imports
    # before an optional fixture test can mask them through an unavailable input.
    for directory in (ROOT / "tools", ROOT / "firmware/tools"):
        for path in directory.glob("*.py"):
            for node in ast.walk(ast.parse(path.read_text())):
                if not isinstance(node, ast.Constant) or not isinstance(node.value, str):
                    continue
                value = node.value
                if not re.fullmatch(r"(?:[A-Za-z0-9_-]+/)*[A-Za-z0-9_-]+\.py", value):
                    continue
                if value.startswith("tools/"):
                    dest = ROOT / value
                    if not dest.exists() and directory == ROOT / "firmware/tools":
                        dest = ROOT / "firmware" / value
                elif "/" not in value:
                    dest = directory / value
                    if not dest.exists():
                        dest = ROOT / "tools" / value
                else:
                    continue
                if not dest.is_file():
                    errors.append(f"{path.relative_to(ROOT)}: missing tool dependency {value}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Repository check passed: {len(paths)} files, {links} local documentation links.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
