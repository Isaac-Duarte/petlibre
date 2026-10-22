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
# FW08 no-dispense rule: network-facing transport code may name only these
# paths (full matches), so nothing reachable from the network can touch
# feeding, outputs, settings, flash or updates. Paths whose first segment is
# capitalized (types, enum variants, Self) are local and always allowed.
TRANSPORT_ISOLATION = {
    "crates/petlibre-core/src/transport.rs": (r"core::(fmt|str)(::\w+)*", r"fmt::\w+"),
    "crates/petlibre-amebad/src/transport.rs": (
        r"core::fmt::Write", r"core::str::from_utf8", r"core::fmt::(Result|Formatter|Display)",
        r"petlibre_core::transport(::\w+)?", r"mqtt::\w+(::\w+)?",
        r"crate::health(::(Boot|Link|self))?", r"health::(snapshot|Link|Boot)(::default)?",
        r"crate::net_status", r"net_status::read",
        r"crate::sdk(::(TcpRecv|TransportConfig|self))?",
        r"sdk::(tcp_connect|tcp_send|tcp_recv|tcp_close|uptime_ms|TcpRecv|TransportConfig)",
    ),
}
PRIMITIVES = {"u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "bool", "char", "str"}
PATH = re.compile(r"\b([A-Za-z_]\w*(?:::\w+)+)")
USE_GROUP = re.compile(r"\buse\s+([\w:]+)::\{([^}]*)\}")


def transport_isolation(errors):
    for name, allowed in TRANSPORT_ISOLATION.items():
        path = ROOT / name
        if not path.is_file():
            errors.append(f"{name}: transport isolation target missing")
            continue
        # Code only: comments may name other modules.
        code = "\n".join(line.split("//", 1)[0] for line in path.read_text().splitlines())
        if re.search(r"\b(?:unsafe|extern|mod|include!|static\s+mut)\b", code):
            errors.append(f"{name}: transport code may not declare modules, externs, unsafe or mutable statics")
        if re.search(r"\buse\s+[^;]*\*", code):
            errors.append(f"{name}: transport code may not glob-import")
        paths = []
        for base, items in USE_GROUP.findall(code):
            for item in items.split(","):
                item = item.strip().split(" as ")[0].strip()
                if item:
                    paths.append(base if item == "self" else f"{base}::{item}")
        code = USE_GROUP.sub("", code)
        paths += PATH.findall(code)
        for found in paths:
            if found[0].isupper() or found.split("::")[0] in PRIMITIVES:
                continue
            if not any(re.fullmatch(rule, found) for rule in allowed):
                errors.append(f"{name}: transport code reaches {found}")


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
    scripts = [p for p in (ROOT / "tools").rglob("*.py") if "tests" not in p.parts and "__pycache__" not in p.parts]
    scripts += list((ROOT / "firmware/tools").glob("*.py"))
    for path in scripts:
        directory = path.parent
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
            elif (ROOT / "tools" / value.split("/")[0]).is_dir():
                dest = ROOT / "tools" / value  # A category-qualified sibling, e.g. "flash/uart-flash-read.py".
            else:
                continue
            if not dest.is_file():
                errors.append(f"{path.relative_to(ROOT)}: missing tool dependency {value}")
    transport_isolation(errors)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Repository check passed: {len(paths)} files, {links} local documentation links.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
