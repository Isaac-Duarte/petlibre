"""Audit flash-write and eFuse-write reachability in the KM4 image (FW07).

Linking the Wi-Fi driver brings in code that can program eFuse, which is
one-time programmable, and the SDK's Wi-Fi flash users default to sectors
inside the application bank. This checks the linked image, not runtime
behavior: references are direct branches (through veneers too) and 32-bit
words anywhere in loaded bytes equal to a function's address, attributed to
the enclosing symbol. Register-computed calls are not visible here.
"""

import bisect
import hashlib
import re

import rampark

# Never in the image: eFuse MAC programming, eFuse-command issuers, the SDK
# Wi-Fi/FTL flash writers, the mbed flash write API and any shell entry.
ABSENT = ("rtw_efuse_boot_write", "wifi_set_mac_address", "wlan_write_reconnect_data_to_flash",
          "restore_wifi_info_to_flash", "example_wlan_fast_connect", "ftl_init", "flash_erase_sector",
          "flash_erase_block", "flash_erase_chip", "flash_stream_write", "flash_write_word",
          "flash_burst_write", "flash_write_protect", "flash_set_status", "shell_init_ram",
          "shell_task_ram", "shell_init_rom", "shell_task_rom", "shell_loop_rom")
WRITES = re.compile(r"^(EFUSE_LogicalMap_Write|EFUSE_PG_Packet|EFUSEWrite8|EFUSE_PMAP_WRITE8|"
                    r"FLASH_(Erase|EraseXIP|EreaseDwordsXIP|SetStatus|SetStatusBits|SetStatusXIP|"
                    r"SetStatusBitsXIP|SetSpiMode|WriteEn|WriteStream|TxData\w*|TxCmd\w*))$")
# Every reference to a write entry, pinned. The RAM *XIP wrappers call their ROM
# counterparts; none of the wrappers but FLASH_EreaseDwordsXIP is referenced,
# and it is not. The driver reaches eFuse only through its private-command
# table, and nothing linked issues an eFuse command (the issuers are absent).
# cmd_flash and cmd_efuse_protect live in the vendor shell command table, which
# the linker script keeps; no shell entry is linked, and the table's only other
# reference is the empty init arrays that share its address.
EXPECTED = {
    "EFUSE_LogicalMap_Write": {"rtw_config_map_write", "cmd_efuse_protect"},
    "EFUSEWrite8": {"cmd_efuse_protect"},
    "rtw_config_map_write": {"rtw_ex_set"},
    "rtw_ex_set": {"rtw_private_handler"},
    "FLASH_Erase": {"FLASH_EraseXIP", "cmd_flash"},
    "FLASH_SetStatus": {"FLASH_SetStatusXIP", "cmd_flash"},
    "FLASH_SetStatusBits": {"FLASH_SetStatusBitsXIP"},
    "FLASH_TxData12B": {"FLASH_TxData12BXIP", "FLASH_WriteStream", "cmd_flash"},
    "FLASH_TxData256B": {"FLASH_TxData256BXIP"},
    "FLASH_EraseXIP": {"FLASH_EreaseDwordsXIP"},
    "FLASH_TxData12BXIP": {"FLASH_EreaseDwordsXIP"},
    "FLASH_EreaseDwordsXIP": set(),
    "cmd_flash": {"shell_cmd_table"},
    "cmd_efuse_protect": {"shell_cmd_table"},
    # The table's help command walks it and is itself only a table entry.
    "shell_cmd_table": {"__libc_init_array", "CmdRamHelp"},
    "CmdRamHelp": {"shell_cmd_table"},
}
BRANCH = re.compile(r"\b(?:bl|blx|b(?:\.w)?)\s+([0-9a-f]+)\s+<([^>+]+)>")


def table(elf, objdump):
    """{name: (address, kind)}; ROM functions come from the absolute symbol file."""
    names = {}
    for line in rampark.tool(objdump, "-t", elf).splitlines():
        fields = line.split()
        if len(fields) >= 5 and re.fullmatch(r"[0-9a-f]{8}", fields[0]):
            name, section = fields[-1], fields[-3]
            if section == "*UND*" or name.startswith("$") or name.startswith("."):
                continue
            names.setdefault(name, (int(fields[0], 16), section))
    return names


def loaded(elf, objdump):
    """(address, bytes) of every section with file contents that loads."""
    data = elf.read_bytes()
    pattern = re.compile(r"^\s*\d+\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)")
    lines = rampark.tool(objdump, "-h", elf).splitlines()
    for index, line in enumerate(lines):
        match = pattern.match(line)
        if match and index + 1 < len(lines) and "LOAD" in lines[index + 1] and "CONTENTS" in lines[index + 1]:
            size, start, offset = (int(x, 16) for x in match.groups()[1:])
            yield start, data[offset:offset + size]


def references(elf, objdump, names, targets):
    """{target: set of enclosing symbols that reference it}."""
    starts = sorted((address & ~1, name) for name, (address, section) in names.items()
                    if section not in ("*ABS*",) and address)
    keys = [address for address, _ in starts]

    def owner(location):
        at = bisect.bisect_right(keys, location) - 1
        return starts[at][1] if at >= 0 else hex(location)
    wanted = {}
    for name in targets:
        address = names[name][0]
        for value in (address | 1, address & ~1):
            wanted[value] = name
    found = {name: set() for name in targets}
    for start, data in loaded(elf, objdump):
        for at in range(0, len(data) - 3, 4):
            value = int.from_bytes(data[at:at + 4], "little")
            if value in wanted:
                found[wanted[value]].add(owner(start + at))
    current = None
    veneers = {}
    listing = rampark.tool(objdump, "-d", "-j", ".ram_image2.text", "-j", ".xip_image2.text", elf)
    for line in listing.splitlines():
        header = re.match(r"^([0-9a-f]+) <([^>]+)>:$", line)
        if header:
            current = header[2]
            continue
        branch = BRANCH.search(line)
        if branch and current:
            target = branch[2]
            if target.startswith("__") and target.endswith("_veneer"):
                veneers.setdefault(target[2:-7], set()).add(current)
            elif target in found and target != current:
                found[target].add(current)
    for target, callers in veneers.items():
        if target in found:
            found[target] |= callers
    for target in found:
        found[target] = {n for n in found[target]
                         if n != target and not (n.startswith("__") and n.endswith("_veneer"))}
    return found


def audit(sdk, project, binutils):
    objdump = binutils / "arm-none-eabi-objdump"
    elf = sdk / project / "project_hp/asdk/image/target_img2.axf"
    names = table(elf, objdump)
    present = sorted(name for name in ABSENT if name in names)
    if present:
        raise ValueError("Forbidden flash/eFuse writer linked: " + ", ".join(present))
    targets = sorted({name for name in names if WRITES.match(name)} | (set(EXPECTED) & set(names)))
    found = references(elf, objdump, names, targets)
    unexpected = {target: sorted(callers - EXPECTED.get(target, set()))
                  for target, callers in found.items() if callers - EXPECTED.get(target, set())}
    if unexpected:
        raise ValueError("Unreviewed flash/eFuse write reference: " + repr(unexpected))
    for target, callers in EXPECTED.items():
        if target in names and found.get(target) != callers:
            raise ValueError("Pinned flash/eFuse reference changed: " + target)
    return {"elf_sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
            "absent": list(ABSENT), "write_targets": targets,
            "references": {k: sorted(v) for k, v in found.items() if v},
            "scope": "Linked direct and stored-address references only; register-computed calls, "
                     "ROM-internal paths and runtime behavior are not established."}


if __name__ == "__main__":
    import json
    import sys
    from pathlib import Path
    print(json.dumps(audit(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])), indent=1))
