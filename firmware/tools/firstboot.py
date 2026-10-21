"""Check the deliberately limited SDK mains in the UART-only build profile.

This checks linked direct calls, not a complete startup or ROM side-effect proof.
"""
import re
import struct

import rampark
import sdkhooks

EXPECTED = {
    "lp": ("irq_register", "irq_enable", "pmu_acquire_wakelock", "irq_disable",
           "km4_flash_highspeed_init", "km4_boot_on", "petlibro_shared_pad_power_init",
           "vTaskStartScheduler"),
    "hp": ("irq_register", "irq_enable", "ipc_table_init", "pmu_acquire_wakelock",
           "pre_example_entry", "example_entry", "vTaskStartScheduler"),
}


def sources(firmware):
    return {core: firmware / "ports/amebad" / f"firstboot_km{number}.c"
            for core, number in (("lp", 0), ("hp", 4))}


def check_main(listing, address, size, core, symbols, veneers=None):
    """Resolve constant ROM calls; reject unknown, missing or reordered calls."""
    calls = []
    instructions = 0
    terminal_loop = False
    registers = {}
    veneers = veneers or {}
    literals = {}
    for line in listing.splitlines():
        match = rampark.INSTRUCTION.match(line)
        if match and match[2] == ".word":
            literals[int(match[1], 16)] = int(match[3].split()[0], 16)

    def called(target):
        names = [name for name in EXPECTED[core] if symbols.get(name, -1) & ~1 == target & ~1]
        name = veneers.get(target & ~1)
        if name is None:
            if len(names) != 1:
                raise ValueError("Unknown/ambiguous first-boot call target")
            name = names[0]
        calls.append(name)
        for register in ("r0", "r1", "r2", "r3", "ip", "lr"):
            registers.pop(register, None)

    for line in listing.splitlines():
        match = rampark.INSTRUCTION.match(line)
        if not match:
            if re.match(r"^\s*[0-9a-f]+:\s", line):
                raise ValueError("Unparsed first-boot instruction")
            continue
        location, mnemonic, operands = match.groups()
        location = int(location, 16)
        if not address <= location < address + size:
            raise ValueError("Instruction outside first-boot main")
        if mnemonic == ".word":
            continue
        if mnemonic not in {"push", "ldr", "ldr.w", "movs", "blx", "bl", "b.n", "nop"}:
            raise ValueError("Unreviewed first-boot instruction: " + mnemonic)
        if terminal_loop and mnemonic != "nop":
            raise ValueError("Instructions after first-boot terminal loop")
        instructions += 1
        if mnemonic == "blx":
            target = registers.get(operands.strip())
            if target is None or not target & 1:
                raise ValueError("Unresolved indirect branch in first-boot main")
            called(target)
            continue
        if mnemonic == "bx":
            raise ValueError("Indirect branch in first-boot main")
        if mnemonic == "bl" or rampark.DIRECT_BRANCH.fullmatch(mnemonic):
            target = re.search(r"^([0-9a-f]+)\s+<([^>]+)>", operands)
            if not target:
                raise ValueError("Unresolved first-boot branch")
            if address <= int(target[1], 16) < address + size:
                if mnemonic != "b.n" or int(target[1], 16) != location:
                    raise ValueError("Unexpected branch inside first-boot main")
                terminal_loop = True
                continue
            called(int(target[1], 16))
            continue
        # Literal loads may read constants; no other PC destination is allowed.
        code = operands.split(";")[0]
        if code.split(",")[0].strip() == "pc" or (mnemonic == "pop" and "pc" in code):
            raise ValueError("Indirect PC write in first-boot main")
        destination = code.split(",")[0].strip()
        if re.fullmatch(r"r(?:[0-9]|1[0-2])|ip|lr", destination):
            registers.pop(destination, None)
            literal = re.fullmatch(r"(r[0-9]+|ip), \[pc, #(\d+)\]\s*", code.strip())
            if mnemonic in ("ldr", "ldr.w") and literal:
                at = ((location + 4) & ~3) + int(literal[2])
                if at not in literals or not address <= at < address + size:
                    raise ValueError("Unresolved first-boot ROM pointer literal")
                registers[destination] = literals[at]
    if not instructions or not terminal_loop or tuple(calls) != EXPECTED[core]:
        raise ValueError(f"Unexpected first-boot {core} main calls: {calls}")
    return {"instructions": instructions, "resolved_calls": calls,
            "unresolved_calls": 0}


def audit(sdk, project, binutils):
    result = {}
    objdump = binutils / "arm-none-eabi-objdump"
    for core in ("lp", "hp"):
        elf = sdk / project / f"project_{core}/asdk/image/target_img2.axf"
        matches = []
        symbols = {}
        for line in rampark.tool(objdump, "-t", elf).splitlines():
            fields = line.split()
            if len(fields) >= 4 and re.fullmatch(r"[0-9a-f]+", fields[0]):
                symbols[fields[-1]] = int(fields[0], 16)
            if len(fields) == 6 and fields[-1] == "main" and fields[2] == "F":
                matches.append((int(fields[0], 16), int(fields[4], 16)))
        if len(matches) != 1:
            raise ValueError("Expected exactly one linked first-boot main")
        address, size = matches[0]
        veneers = {}
        for name in EXPECTED[core]:
            veneer_name = "__" + name + "_veneer"
            if veneer_name in symbols:
                at = symbols[veneer_name]
                offset = sdkhooks.file_offset(elf, objdump, at, 16)
                expected = bytes.fromhex("01b40248844601bc604700bf") + struct.pack("<I", symbols[name] | 1)
                if elf.read_bytes()[offset:offset + 16] != expected:
                    raise ValueError("Unreviewed first-boot veneer instructions/target")
                veneers[at] = name
        listing = rampark.tool(objdump, "-d", "--start-address=" + hex(address),
                               "--stop-address=" + hex(address + size), elf)
        result[core] = {"address": hex(address), "bytes": size,
                        **check_main(listing, address, size, core, symbols, veneers)}
    return {"mains": result, "network_or_shell_started_by_main": False,
            "feeder_backend_opened_by_application": False,
            "scope": "Exact linked main call sequences, constant ROM pointers and linker veneers; vendor pre-main, IRQ handlers, "
                     "flash status/calibration, shared-pad supply helper and ROM effects require separate review"}
