#!/usr/bin/env python3
"""Independent expected development-update TX; no installer imports or I/O."""
import hashlib
import struct

START, SECTOR, LIMIT = 0x186000, 4096, 0x150000


def expected(source, target, current, *, include_acquisitions=True):
    def unpack(image):
        if type(image) is not bytes or len(image) < 32:
            raise ValueError("Complete immutable OTA required")
        v, n, magic, entry, check, size, offset, address = struct.unpack_from("<8I", image)
        data = image[32:]
        if (v, n, magic, entry, offset, address) != (0xffffffff, 1, 0x3141544f, 24, 32, 0xffffffff) or \
                size != len(data) or not 0 < size <= LIMIT or size % SECTOR or sum(data) & 0xffffffff != check:
            raise ValueError("Unsupported application container")
        return data

    old, new = unpack(source), unpack(target)
    if type(current) is not bytes or len(current) != max(len(old), len(new)) or current[:len(old)] != old:
        raise ValueError("Source union mismatch")
    if type(include_acquisitions) is not bool:
        raise ValueError("Explicit acquisition profile required")
    result = new + current[len(new):]
    stage = b"\xff" * 8 + result[8:]

    def read(at, size):
        return b"\x20" + at.to_bytes(3, "little") + (size // SECTOR).to_bytes(2, "little") + b"\x06" * (size // 1024 + 1)

    def packet(n, at, data):
        body = struct.pack("<I", 0x08000000 + at) + data
        return bytes([2, n, n ^ 255]) + body + bytes([sum(body) & 255])

    acquire = read(0, 0x6000) + read(0x6000, SECTOR) + read(START, len(current)) if include_acquisitions else b""
    parts = [acquire]
    if result == current:
        return b"".join(parts)
    status = b"".join(bytes([0x21, opcode, 1]) for _ in range(2) for opcode in (5, 0x35, 0x15))
    for i in range(0, len(current), SECTOR):
        data = stage[i:i + SECTOR]
        if data == current[i:i + SECTOR]:
            continue
        at = START + i
        parts.extend([read(at, SECTOR), status, b"\x17" + at.to_bytes(3, "little") + b"\x01\x00",
                      read(at, SECTOR), status, b"\x07"])
        parts.extend(packet(n + 1, at + n * 1024, data[n * 1024:(n + 1) * 1024]) for n in range(4))
        parts.extend([b"\x04", read(at, SECTOR)])
    parts.extend([acquire, read(START, SECTOR), status, b"\x07", packet(1, START, result[:1024]),
                  b"\x04", read(START, SECTOR), acquire])
    return b"".join(parts)
