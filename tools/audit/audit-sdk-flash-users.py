#!/usr/bin/env python3
"""Read-only inventory of visible flash/IPC branches in both linked SDK images.

This is a call-site inventory, never a proof that all storage users are mediated.
Indirect calls, ROM behavior and runtime ownership require separate evidence.
"""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[2]
LOCK = json.loads((ROOT / "firmware/sdk.lock.json").read_text())
DEFAULT_TOOLS = ROOT / "private/toolchains" / LOCK["toolchain"]["directory"] / "bin"
PROJECT = Path("sdk/project/realtek_amebaD_va0_example/GCC-RELEASE")
HEADER = re.compile(r"^([0-9a-f]+) <([^>]+)>:$")
INSTRUCTION = re.compile(r"^\s*([0-9a-f]+):\s+(?:(?:[0-9a-f]{4}|[0-9a-f]{8})\s+)+([a-z][a-z0-9.]*)\s*(.*)$")
BRANCH = re.compile(r"b(?:l|lx|x|eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[nw])?$")
DIRECT = re.compile(r"(?:0x)?([0-9a-f]+)\s+<([^>]+)>")
INDIRECT = re.compile(r"(?:r(?:[0-9]|1[0-2])|ip|sp|pc)(?:\s|$)")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tool(executable, *args):
    return subprocess.run([str(executable), *map(str, args)], check=True,
                          text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def relevant(name):
    return "+" not in name and name.startswith(("FLASH_", "flash_", "IPC_", "ipc_"))


def verify_packaged_code(elf, objdump, payload, regions, core):
    """Reject stale ELF text even if older packaged artifacts still match."""
    data = elf.read_bytes()
    checked = []
    pattern = re.compile(r"^\s*\d+\s+(\.(?:ram|xip)_image2\.text)\s+"
                         r"([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)")
    for line in tool(objdump, "-h", elf).splitlines():
        match = pattern.match(line)
        if not match:
            continue
        name, size, address, offset = match.groups()
        size, address, offset = int(size, 16), int(address, 16), int(offset, 16)
        region_name = core.lower() + ("_ram" if name.startswith(".ram") else "_xip")
        region = next(r for r in regions if r["name"] == region_name)
        relative = address - int(region["runtime_address"], 16)
        start = region["payload_header_offset"] + 32 + relative
        if size <= 0 or offset + size > len(data) or relative < 0 or relative + size > region["body_bytes"]:
            raise ValueError("ELF text is outside packaged region: " + name)
        if data[offset:offset + size] != payload[start:start + size]:
            raise ValueError("ELF text does not match packaged image: " + name)
        checked.append({"section": name, "address": f"0x{address:08x}", "bytes": size,
                        "sha256": hashlib.sha256(data[offset:offset + size]).hexdigest()})
    if {x["section"] for x in checked} != {".ram_image2.text", ".xip_image2.text"} or len(checked) != 2:
        raise ValueError("Expected exactly the packaged XIP and RAM text sections")
    return checked


def inventory(elf, nm, objdump):
    symbols = {}
    for line in tool(nm, "--defined-only", elf).splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[1] in "TtWwAa":
            symbols[parts[2]] = int(parts[0], 16) & ~1
    disassembly = tool(objdump, "-d", "-j", ".ram_image2.text", "-j", ".xip_image2.text", elf)
    functions = {}
    name = None
    for line in disassembly.splitlines():
        header = HEADER.match(line)
        if header:
            if "+" not in header[2]:
                name = header[2]
                functions[name] = {"address": int(header[1], 16), "lines": []}
            continue
        if name is not None:
            functions[name]["lines"].append(line)

    edges, indirect = [], []
    for caller, body in functions.items():
        for line in body["lines"]:
            instruction = INSTRUCTION.match(line)
            if not instruction or not BRANCH.fullmatch(instruction[2]):
                continue
            address, mnemonic, operands = instruction.groups()
            target = DIRECT.match(operands)
            if target:
                callee = target[2]
                via = None
                literal_matches = None
                candidate = re.fullmatch(r"__(.+)_veneer", callee)
                if candidate:
                    via, callee = callee, candidate[1]
                    lines = functions.get(via, {}).get("lines", [])
                    literals = [int(value, 16) & ~1 for value in re.findall(
                        r"\.word\s+0x([0-9a-f]+)", "\n".join(lines))]
                    literal_matches = callee in symbols and symbols[callee] in literals
                if relevant(callee):
                    edges.append({"caller": caller, "address": "0x" + address,
                                  "instruction": mnemonic, "target": "0x" + target[1],
                                  "callee": callee, "via_veneer": via,
                                  "veneer_literal_matches_symbol": literal_matches})
            elif mnemonic in ("blx", "bx") and INDIRECT.match(operands):
                indirect.append({"caller": caller, "address": "0x" + address,
                                 "instruction": mnemonic + " " + operands})
    return {"elf": str(elf), "bytes": elf.stat().st_size, "sha256": digest(elf),
            "relevant_symbols": {name: f"0x{address:08x}" for name, address in sorted(symbols.items())
                                 if relevant(name)}, "visible_branches": edges,
            "unresolved_register_branches": indirect,
            "complete_call_graph": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=Path, default=ROOT / "private/firmware-build/plaf103")
    parser.add_argument("--binutils", type=Path, default=DEFAULT_TOOLS)
    args = parser.parse_args()
    args.binutils = args.binutils.resolve()
    build = args.build.resolve()
    manifest_path = build / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    # Bind the inventory to current output artifacts, not a manifest left after
    # a newer partial build. ELF text is also matched to packaged bytes below.
    for artifact in manifest["artifacts"]:
        path = (build / artifact["path"]).resolve()
        if not path.is_relative_to(build) or digest(path) != artifact["sha256"]:
            raise ValueError("Build artifact does not match manifest: " + artifact["path"])
    spec = importlib.util.spec_from_file_location("ota_layout", ROOT / "tools/audit/audit-ota-layout.py")
    layout = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(layout)
    ota = next(item for item in manifest["artifacts"] if Path(item["path"]).name == "OTA_All.bin")
    payload = layout.container((build / ota["path"]).read_bytes())
    regions = layout.regions(payload)
    result = {"schema_version": 1, "sdk_revision": manifest["sdk_revision"],
              "build_manifest": str(manifest_path), "manifest_sha256": digest(manifest_path),
              "binutils": {name: {"path": str(args.binutils / name),
                                  "sha256": digest(args.binutils / name)}
                           for name in ("arm-none-eabi-nm", "arm-none-eabi-objdump")},
              "images": {}, "scope": "Visible linked branches and unresolved register branches; "
              "packaged XIP/RAM text is byte-matched to the ELF; "
              "veneer names are candidates with a separate literal check, not full dataflow. "
              "No proof of global flash exclusion or device ROM compatibility."}
    for core, project in (("KM0", "project_lp"), ("KM4", "project_hp")):
        elf = build / PROJECT / project / "asdk/image/target_img2.axf"
        checked = verify_packaged_code(elf, args.binutils / "arm-none-eabi-objdump", payload, regions, core)
        result["images"][core] = inventory(elf, args.binutils / "arm-none-eabi-nm",
                                          args.binutils / "arm-none-eabi-objdump")
        result["images"][core]["packaged_text_verified"] = checked
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
