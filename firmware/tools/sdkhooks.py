"""Overlay SDK hook definitions and audit public/legacy routing in both ELFs.

This proves the selected linked hook routes, not the behavior of an installed
protocol provider or complete mediation of raw ROM/indirect flash users.
"""

import hashlib
from pathlib import Path
import re
import struct

import rampark

SOURCE = Path("component/soc/realtek/amebad/fwlib/ram_common/rtl8721d_flash_ram.c")
RENAMES = {"FLASH_Write_Lock": "petlibre_sdk_flash_legacy_lock",
           "FLASH_Write_Unlock": "petlibre_sdk_flash_legacy_unlock",
           "FLASH_Write_IPC_Int": "petlibre_sdk_flash_legacy_ipc"}
FUNCTIONS = set(RENAMES) | set(RENAMES.values()) | {
    "petlibre_amebad_flash_hooks_install", "petlibre_amebad_flash_hooks_armed", "hook_fail"}
STATE = {"hook_transport", "hook_ticket", "hook_last_id", "hook_armed", "hook_faulted",
         "hook_legacy_locked", "hook_lease_held"}


def overlay(source):
    for public, legacy in RENAMES.items():
        pattern = r"(?m)^(void\s+)" + re.escape(public) + r"(\s*\()"
        source, count = re.subn(pattern, lambda m: m[1] + legacy + m[2], source)
        if count != 1:
            raise ValueError("Expected exactly one pinned SDK definition: " + public)
    return source


def symbols(elf, objdump):
    selected = {}
    wanted = FUNCTIONS | STATE | {"ipc_init_config"}
    for line in rampark.tool(objdump, "-t", elf).splitlines():
        fields = line.split()
        if len(fields) == 6 and fields[-1] in wanted:
            address, _, kind, section, size, name = fields
            if name in selected:
                raise ValueError("Ambiguous SDK hook symbol: " + name)
            selected[name] = {"address": int(address, 16), "bytes": int(size, 16),
                              "kind": kind, "section": section}
    if set(selected) != wanted:
        raise ValueError("Missing SDK hook symbols: " + ", ".join(sorted(wanted - set(selected))))
    return selected


def file_offset(elf, objdump, address, size):
    pattern = re.compile(r"^\s*\d+\s+\.\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)")
    data = elf.read_bytes()
    for line in rampark.tool(objdump, "-h", elf).splitlines():
        match = pattern.match(line)
        if match:
            length, start, offset = (int(x, 16) for x in match.groups())
            if start <= address and address + size <= start + length:
                at = offset + address - start
                if at + size <= len(data):
                    return at
    raise ValueError("Hook address is not in initialized ELF bytes")


def verify_ipc_table(elf, objdump, names):
    # Pinned IPC_INIT_TABLE = three 32-bit members. Channel 2 function is
    # entry offset 2*12+4; require the new public Thumb entry, not legacy.
    table = names["ipc_init_config"]
    if table["bytes"] < 36:
        raise ValueError("IPC table is too short")
    at = file_offset(elf, objdump, table["address"] + 28, 4)
    pointer, = struct.unpack_from("<I", elf.read_bytes(), at)
    if pointer != (names["FLASH_Write_IPC_Int"]["address"] | 1):
        raise ValueError("Flash IPC table bypasses the public hook")
    return pointer


def legacy_routes(disassembly):
    expected = {legacy: public for public, legacy in RENAMES.items()}
    routes = {legacy: [] for legacy in expected}
    owner = None
    for line in disassembly.splitlines():
        heading = re.match(r"^([0-9a-f]+) <([^>]+)>:$", line)
        if heading:
            owner = heading[2]
            continue
        instruction = rampark.INSTRUCTION.match(line)
        if instruction and (rampark.DIRECT_BRANCH.fullmatch(instruction[2]) or instruction[2] == "bl"):
            target = re.search(r"\b[0-9a-f]+\s+<([^>]+)>", instruction[3])
            if target and target[1] in expected:
                if owner != expected[target[1]]:
                    raise ValueError("SDK caller bypasses hook: " + str(owner) + " -> " + target[1])
                routes[target[1]].append({"caller": owner, "address": "0x" + instruction[1]})
    if any(not calls for calls in routes.values()):
        raise ValueError("Missing visible wrapper route to legacy hook")
    return routes


def audit(sdk, project, binutils, objects):
    """`objects` maps KM0/KM4 to the linked Rust archive containing this code."""
    objdump = binutils / "arm-none-eabi-objdump"
    result = {}
    for core, directory, objdir in (("KM0", "lp", "project/src"), ("KM4", "hp", "utilities_example")):
        base = sdk / project / ("project_" + directory) / "asdk"
        elf = base / "image/target_img2.axf"
        names = symbols(elf, objdump)
        for name in FUNCTIONS:
            if names[name]["kind"] != "F" or names[name]["section"] != ".ram_image2.text":
                raise ValueError("SDK hook function is outside application RAM: " + name)
        for name in STATE:
            if names[name]["kind"] != "O" or names[name]["section"] != ".ram_image2.bss":
                raise ValueError("SDK hook state is outside internal application RAM: " + name)
        pointer = verify_ipc_table(elf, objdump, names)
        listing = rampark.tool(objdump, "-d", "-j", ".ram_image2.text", "-j", ".xip_image2.text", elf)
        routes = legacy_routes(listing)
        obj = objects[core]
        result[core] = {"elf_sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
                        "object_path": str(obj.relative_to(sdk.parent)),
                        "object_sha256": hashlib.sha256(obj.read_bytes()).hexdigest(),
                        "symbols": names, "legacy_entry_routes": routes,
                        "ipc_channel_2_handler": hex(pointer)}
    return {"images": result, "protocol_installed": False, "global_exclusion_proven": False,
            "scope": "Public SDK hooks and IPC table route through RAM interposer; "
                     "legacy entry branches originate only in that interposer. "
                     "Indirect callers, provider implementation and global exclusion are unproven."}
