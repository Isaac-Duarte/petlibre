"""Reproduce and audit the reviewed stock KM0 pad table without device I/O."""

import hashlib
import re
import struct


STOCK_SHA256 = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498"
STOCK_BYTES = 1376288
TABLE_ADDRESS = 0x0C013328
XIP_ADDRESS = 0x0C000020
TABLE_ROWS = 65
ROW_BYTES = 20
CHANGED_PINS = (12, 13, 14, 15, 18, 30, 33, 34)
PULLS = {"GPIO_PuPd_NOPULL": 0, "GPIO_PuPd_DOWN": 1, "GPIO_PuPd_UP": 2,
         "GPIO_PuPd_SHUTDOWN": 3, "GPIO_PuPd_KEEP": 255}
FLAGS = {"FALSE": 0, "TRUE": 1}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def stock_table(reference):
    require(len(reference) == STOCK_BYTES and
            hashlib.sha256(reference).hexdigest() == STOCK_SHA256,
            "Pin table requires the exact verified stock OTA")
    start = 64 + TABLE_ADDRESS - XIP_ADDRESS
    data = reference[start:start + TABLE_ROWS * ROW_BYTES]
    rows = list(struct.iter_unpack("<5I", data))
    require([r[0] for r in rows] == list(range(64)) + [0xFFFFFFFF],
            "Unexpected stock pin order or sentinel")
    return data, rows


def overlay(source, reference):
    """Patch eight active pulls; refuse any other discrepancy with stock."""
    _, expected = stock_table(reference)
    tables = list(re.finditer(r"const\s+PMAP_TypeDef\s+pmap_func\[\]\s*=\s*"
                              r"\{(?P<body>.*?)^\};", source, re.MULTILINE | re.DOTALL))
    require(len(tables) == 1, "Expected one pinned SDK pmap_func table")
    table = tables[0]
    row_pattern = re.compile(r"^[ \t]*\{[ \t]*(_P[AB]_\d+|_PNC)[ \t]*,[ \t]*"
                             r"([^{}\r\n]+)\},[^\r\n]*$")
    output, rows, changed = [], [], []
    for line in table['body'].splitlines(keepends=True):
        match = row_pattern.fullmatch(line.rstrip("\r\n"))
        if match:
            name = match[1]
            pin = 0xFFFFFFFF if name == "_PNC" else (
                (32 if name[2] == "B" else 0) + int(name[4:]))
            values = [x.strip() for x in match[2].split(",")]
            require(len(values) == 4 and all(x in PULLS for x in values[:3]) and
                    values[3] in FLAGS, "Unrecognized SDK pad fields")
            row = (pin, *(PULLS[x] for x in values[:3]), FLAGS[values[3]])
            index = len(rows)
            require(index < TABLE_ROWS and row[0] == expected[index][0],
                    "SDK pin order or sentinel differs from stock")
            target = expected[index]
            require(row[2:] == target[2:], "SDK sleep/low-power fields differ from stock")
            if row[1] != target[1]:
                require(pin in CHANGED_PINS and target[1] == 0,
                        "Unexpected active pull discrepancy")
                line, count = re.subn(r"^([^,]+,[ \t]*)GPIO_PuPd_[A-Z]+",
                                     r"\g<1>GPIO_PuPd_NOPULL", line)
                require(count == 1, "Cannot replace SDK active pull")
                changed.append(pin)
            rows.append(target)
        output.append(line)
    require(len(rows) == TABLE_ROWS and tuple(changed) == CHANGED_PINS,
            "Expected 65 records and the eight reviewed active pull changes")
    return source[:table.start('body')] + ''.join(output) + source[table.end('body'):]


def audit_linked_table(candidate, address, reference):
    """Check actual linked bytes; source text alone cannot detect stale objects."""
    expected, _ = stock_table(reference)
    require(len(candidate) >= 64, "Candidate KM0 image header is truncated")
    magic1, magic2, size, base = struct.unpack_from("<4I", candidate, 32)
    require((magic1, magic2, base) == (0x35393138, 0x31313738, XIP_ADDRESS),
            "Unexpected candidate KM0 XIP header")
    offset = address - base
    require(0 <= offset <= size - len(expected) and 64 + size <= len(candidate),
            "Linked pad table is outside the KM0 XIP body")
    actual = candidate[64 + offset:64 + offset + len(expected)]
    require(actual == expected, "Linked KM0 pad table differs from verified stock")
    return {"runtime_address": f"0x{address:08x}", "rows": TABLE_ROWS,
            "bytes": len(actual), "sha256": hashlib.sha256(actual).hexdigest(),
            "matches_stock": True,
            "scope": "Pad pull table only; reset levels, GPIO direction and physical behavior unverified"}
