"""Audit flash-write and eFuse-write reachability in both core images (FW07).

Linking the Wi-Fi driver brings in code that can program eFuse, which is
one-time programmable, and the SDK's Wi-Fi flash users default to sectors
inside the application bank. KM0 now also links the WLAN firmware
(lib_wifi_fw.a). This checks the linked images, not runtime
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
EXPECTED_HP = {
    "EFUSE_LogicalMap_Write": {"rtw_config_map_write", "cmd_efuse_protect"},
    "EFUSEWrite8": {"cmd_efuse_protect"},
    "rtw_config_map_write": {"rtw_ex_set"},
    "rtw_ex_set": {"rtw_private_handler"},
    "FLASH_Erase": {"FLASH_EraseXIP", "cmd_flash"},
    "FLASH_SetStatus": {"FLASH_SetStatusXIP", "cmd_flash"},
    "FLASH_SetStatusBits": {"FLASH_SetStatusBitsXIP"},
    "FLASH_TxData12B": {"FLASH_TxData12BXIP", "FLASH_WriteStream", "cmd_flash"},
    "FLASH_TxData256B": {"FLASH_TxData256BXIP"},
    # FW10: the recovery updater's bank-1 wrappers, which refuse unless the
    # image runs from bank 0 and stay inside bank 1 (amebad-sdk target.rs).
    "FLASH_EraseXIP": {"FLASH_EreaseDwordsXIP", "petlibre_amebad_sdk::target::bank1_erase"},
    "FLASH_TxData256BXIP": {"petlibre_amebad_sdk::target::bank1_program"},
    "FLASH_TxData12BXIP": {"FLASH_EreaseDwordsXIP"},
    "FLASH_EreaseDwordsXIP": set(),
    "cmd_flash": {"shell_cmd_table"},
    "cmd_efuse_protect": {"shell_cmd_table"},
    # The table's help command walks it and is itself only a table entry.
    "shell_cmd_table": {"__libc_init_array", "CmdRamHelp"},
    "CmdRamHelp": {"shell_cmd_table"},
}
# KM0 with the WLAN firmware linked: lib_wifi_fw.a itself imports no flash,
# eFuse or shell symbol. The SDK's flash setup (flash_operation_config,
# flash_rx_mode_switch) writes the status register at boot, before KM4 runs.
# The ROM shell is a symbol only; nothing references its entries.
EXPECTED_LP = {
    "EFUSE_LogicalMap_Write": {"cmd_efuse_protect"},
    "EFUSEWrite8": {"cmd_efuse_extpath_write", "cmd_efuse_protect"},
    "EFUSE_PG_Packet": set(),
    "FLASH_Erase": {"FLASH_EraseXIP"},
    "FLASH_SetStatus": {"FLASH_SetStatusXIP", "flash_operation_config", "flash_rx_mode_switch"},
    "FLASH_SetStatusBits": {"FLASH_SetStatusBitsXIP"},
    "FLASH_TxData12B": {"FLASH_TxData12BXIP", "FLASH_WriteStream"},
    "FLASH_TxData256B": {"FLASH_TxData256BXIP"},
    "FLASH_EraseXIP": {"FLASH_EreaseDwordsXIP"},
    "FLASH_TxData12BXIP": {"FLASH_EreaseDwordsXIP"},
    "FLASH_EreaseDwordsXIP": set(),
    "FLASH_WriteStream": set(),
    "cmd_flash": {"shell_cmd_table"},
    "cmd_efuse_protect": {"shell_cmd_table"},
    "cmd_efuse_extpath_write": {"cmd_efuse_protect"},
    "shell_cmd_table": {"CmdRamHelp"},
    "CmdRamHelp": {"shell_cmd_table"},
    "shell_init_rom": set(),
    "shell_task_rom": set(),
}
# KM0 needs the WLAN firmware entry for FW07; KM4 must not carry it.
CORES = {"hp": (ABSENT, EXPECTED_HP, (".ram_image2.text", ".xip_image2.text"), "wifi_FW_init_ram", False),
         "lp": (tuple(n for n in ABSENT if n not in EXPECTED_LP), EXPECTED_LP,
                (".ram_image2.text", ".xip_image2.text", ".ram_retention.text"), "wifi_FW_init_ram", True)}
# Calls and branches, conditional ones included (`bleq` inside an IT block).
BRANCH = re.compile(r"\b(?:blx?|b)(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le|al)?(?:\.[wn])?"
                    r"\s+([0-9a-f]+)\s+<([^>+]+)>")


def demangle(name):
    """A Rust v0 nested path (`_RNvNt...`) as `crate::module::item`, without the
    per-build crate hash, so pins survive rebuilds; other names unchanged."""
    if not name.startswith("_R"):
        return name
    text, at = name[2:], 0

    def ident():
        nonlocal at
        match = re.match(r"u?(\d+)_?", text[at:])
        if not match:
            raise ValueError
        at += match.end()
        value = text[at:at + int(match[1])]
        at += int(match[1])
        return value

    def path():
        nonlocal at
        tag = text[at:at + 1]
        at += 1
        if tag == "C":
            disambiguator = re.match(r"s[0-9A-Za-z]*_", text[at:])
            if disambiguator:
                at += disambiguator.end()
            return [ident()]
        if tag == "N":
            at += 1  # Namespace: v (value), t (type) or another.
            return path() + [ident()]
        raise ValueError
    try:
        parts = path()
    except ValueError:
        return name
    return "::".join(parts) if at == len(text) else name


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


def references(elf, objdump, names, targets, sections):
    """{target: set of enclosing symbols that reference it}."""
    starts = sorted((address & ~1, demangle(name)) for name, (address, section) in names.items()
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
    listing = rampark.tool(objdump, "-d", *(a for s in sections for a in ("-j", s)), elf)
    for line in listing.splitlines():
        header = re.match(r"^([0-9a-f]+) <([^>]+)>:$", line)
        if header:
            current = demangle(header[2])
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


def audit_core(elf, objdump, absent, expected, sections, required, present):
    names = table(elf, objdump)
    linked = sorted(name for name in absent if name in names)
    if linked:
        raise ValueError("Forbidden flash/eFuse writer linked: " + ", ".join(linked))
    if (required in names) != present:
        raise ValueError(("Missing " if present else "Unexpected ") + required)
    targets = sorted({name for name in names if WRITES.match(name)} | (set(expected) & set(names)))
    found = references(elf, objdump, names, targets, sections)
    unexpected = {target: sorted(callers - expected.get(target, set()))
                  for target, callers in found.items() if callers - expected.get(target, set())}
    if unexpected:
        raise ValueError("Unreviewed flash/eFuse write reference: " + repr(unexpected))
    for target, callers in expected.items():
        if target in names and found.get(target) != callers:
            raise ValueError("Pinned flash/eFuse reference changed: " + target)
    return {"elf_sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
            "absent": list(absent), "write_targets": targets,
            "references": {k: sorted(v) for k, v in found.items() if v}}


def audit(sdk, project, binutils):
    objdump = binutils / "arm-none-eabi-objdump"
    result = {core: audit_core(sdk / project / f"project_{core}/asdk/image/target_img2.axf", objdump, *spec)
              for core, spec in CORES.items()}
    return {**result, "scope": "Linked direct and stored-address references only; register-computed calls, "
                               "ROM-internal paths (including the WLAN firmware's ROM half) and runtime behavior "
                               "are not established."}


if __name__ == "__main__":
    import json
    import sys
    from pathlib import Path
    print(json.dumps(audit(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])), indent=1))
