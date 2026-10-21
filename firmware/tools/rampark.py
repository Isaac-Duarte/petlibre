"""Audit the retained, leaf MMIO parking routines in both packaged core images.

This checks emitted instructions and placement, not MMIO ownership, boot readiness,
interrupt/fault-vector behavior or global SDK flash interception.
"""

import hashlib
import importlib.util
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
SYMBOLS = ("petlibro_amebad_flash_park_publish", "petlibro_amebad_flash_park_wait",
           "petlibro_amebad_flash_sem_try_claim", "petlibro_amebad_flash_sem_release",
           "petlibro_amebad_flash_owner_begin", "petlibro_amebad_flash_owner_end")
INSTRUCTION = re.compile(r"^\s*([0-9a-f]+):\s+(?:(?:[0-9a-f]{4}|[0-9a-f]{8})\s+)+([a-z.][a-z0-9.]*)\s*(.*)$")
DIRECT_BRANCH = re.compile(r"b(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[nw])?$")
INTEGER = set("adc adcs add adds addw and ands asr asrs bic bics clz cmn cmp eor eors "
              "ldm ldmia ldr ldrb ldrd ldrh ldrsb ldrsh lsl lsls lsr lsrs mov movs "
              "movw movt mul muls mvn mvns negs nop orr orrs pop push rev rev16 ror rors "
              "rsb rsbs sbc sbcs stm stmia stmdb str strb strd strh sub subs subw tst uxtb uxth".split())


def tool(path, *args):
    return subprocess.run([str(path), *map(str, args)], check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def check_leaf(disassembly, address, size, minimum_barriers=3):
    """Fail closed on calls, foreign branches, indirect jumps or XIP literals."""
    count = barriers = 0
    for line in disassembly.splitlines():
        match = INSTRUCTION.match(line)
        if not match:
            if re.match(r"^\s*[0-9a-f]+:\s", line):
                raise ValueError("Unparsed parking instruction/data")
            continue
        location, mnemonic, operands = match.groups()
        location = int(location, 16)
        if not address <= location < address + size:
            raise ValueError("Instruction outside expected function")
        if mnemonic == ".word":
            value = int(operands.split()[0], 16)
            if 0x0c000000 <= value < 0x10000000:
                raise ValueError("XIP literal in RAM parking code")
            continue
        base = mnemonic.split(".")[0]
        conditional = base[:-2] if base[-2:] in ("eq", "ne", "cs", "hs", "cc", "lo", "mi", "pl",
                                                        "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le") else base
        branch = DIRECT_BRANCH.fullmatch(mnemonic) or base in ("cbz", "cbnz")
        # Conditional returns (`bxeq lr` inside an IT block) are still leaf returns.
        if not (base in INTEGER or conditional in INTEGER or branch or
                base in ("bx", "mrs", "dsb") or conditional == "bx" or re.fullmatch(r"it[te]{0,3}", base)):
            raise ValueError("Unsupported instruction/call in RAM parking code: " + line)
        if conditional == "bx" and operands.strip() != "lr":
            raise ValueError("Indirect branch in RAM parking code")
        code_operands = operands.split(";")[0]
        if code_operands.split(",")[0].strip() == "pc":
            raise ValueError("Indirect PC destination in RAM parking code")
        pc = re.search(r"\bpc\b", code_operands)
        returns = base == "pop" or (base.startswith("ldm") and code_operands.startswith("sp!,"))
        literal_load = base.startswith("ldr") and "[pc," in code_operands
        if pc and not (returns or literal_load):
            raise ValueError("Indirect PC write in RAM parking code")
        if base == "mrs" and operands.split(",")[-1].strip().lower() != "primask":
            raise ValueError("Unexpected system-register access")
        if branch:
            target = re.search(r"\b([0-9a-f]+)\s+<", operands)
            if not target or not address <= int(target.group(1), 16) < address + size:
                raise ValueError("Branch leaves RAM parking function")
        if "[pc," in operands:
            literal = re.search(r";\s*\(?([0-9a-f]+)\s+<", operands)
            if not literal or not address <= int(literal.group(1), 16) < address + size:
                raise ValueError("Literal load leaves RAM parking function")
        count += 1
        barriers += mnemonic == "dsb"
    if count == 0 or barriers < minimum_barriers:
        raise ValueError("Missing parking instructions/barriers")
    return {"instructions": count, "barriers": barriers, "calls": 0,
            "foreign_branches": 0, "xip_literals": 0}


def audit(sdk, project, binutils, objects):
    """`objects` maps KM0/KM4 to the linked Rust archive containing this code."""
    users = module("rampark_flash_users", "tools/audit-sdk-flash-users.py")
    layout = module("rampark_ota_layout", "tools/audit-ota-layout.py")
    image = sdk / project / "project_hp/asdk/image/OTA_All.bin"
    payload = layout.container(image.read_bytes())
    regions = layout.regions(payload)
    result = {}
    for core, directory, objdir in (("KM0", "lp", "project/src"), ("KM4", "hp", "utilities_example")):
        base = sdk / project / ("project_" + directory) / "asdk"
        elf = base / "image/target_img2.axf"
        paired = users.verify_packaged_code(elf, binutils / "arm-none-eabi-objdump", payload, regions, core)
        symbols = {}
        for line in tool(binutils / "arm-none-eabi-objdump", "-t", elf).splitlines():
            fields = line.split()
            if fields and fields[-1] in SYMBOLS and len(fields) == 6:
                address, _, kind, section, size, name = fields
                if kind != "F" or section != ".ram_image2.text":
                    raise ValueError("Parking function is not in application RAM: " + name)
                symbols[name] = (int(address, 16), int(size, 16))
        if set(symbols) != set(SYMBOLS):
            raise ValueError("Missing retained parking functions in " + core)
        checked = {}
        for name, (address, size) in symbols.items():
            listing = tool(binutils / "arm-none-eabi-objdump", "-d",
                           "--start-address=" + hex(address), "--stop-address=" + hex(address + size), elf)
            minimum = 1 if "_sem_" in name else 3
            checked[name] = {"address": hex(address), "bytes": size,
                             **check_leaf(listing, address, size, minimum)}
        obj = objects[core]
        result[core] = {"elf_sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
                        "object_path": str(obj.relative_to(sdk.parent)),
                        "object_sha256": hashlib.sha256(obj.read_bytes()).hexdigest(),
                        "packaged_text_verified": paired, "functions": checked}
    return {"images": result, "activated": False, "global_exclusion_proven": False,
            "scope": "Retained leaf code, local branches/literals and packaged RAM placement; "
                     "runtime pointers, MMIO ownership, vectors, reset and SDK mediation require further evidence"}
