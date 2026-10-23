#!/usr/bin/env python3
"""Audit the known PLAF103 OTA container/placement profile offline; never writes a device."""

import argparse
import hashlib
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[3]
STOCK_SHA256 = "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498"
IMAGE_MAGIC = (0x35393138, 0x31313738)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def container(data):
    require(len(data) >= 32, "OTA header is truncated")
    header = struct.unpack_from("<8I", data)
    version, count, magic, entry_bytes, checksum, size, offset, address = header
    require((version, count, magic, entry_bytes, offset, address) ==
            (0xFFFFFFFF, 1, int.from_bytes(b"OTA1", "little"), 24, 32, 0xFFFFFFFF),
            "Unsupported OTA container profile")
    require(size > 0 and size == len(data) - 32, "OTA payload size does not match EOF")
    payload = data[32:]
    require(sum(payload) & 0xFFFFFFFF == checksum, "OTA byte-sum checksum mismatch")
    return payload


def regions(payload):
    result = []
    offset = 0
    profile = [("km0_xip", 0x0C000020), ("km0_ram", 0x00083000),
               ("km4_xip", 0x0E000020), ("km4_ram", 0x10005000),
               ("km4_psram", 0x02000020)]
    for name, address in profile:
        if name == "km4_xip":
            aligned = (offset + 4095) & ~4095
            require(aligned <= len(payload) and
                    payload[offset:aligned] == b"\xff" * (aligned - offset),
                    "Unexpected inter-core padding")
            offset = aligned
        require(offset + 32 <= len(payload), f"Missing {name} image header")
        words = struct.unpack_from("<8I", payload, offset)
        require(words[:2] == IMAGE_MAGIC and words[3] == address and
                words[4:] == (0xFFFFFFFF,) * 4, f"Unsupported {name} header/load address")
        body_bytes = words[2]
        require((name == "km4_psram" and body_bytes == 0) or
                (name != "km4_psram" and body_bytes > 0), f"Unsupported {name} body size")
        require(offset + 32 + body_bytes <= len(payload), f"{name} body exceeds payload")
        result.append({"name": name, "payload_header_offset": offset,
                       "body_bytes": body_bytes, "runtime_address": f"0x{address:08x}"})
        offset += 32 + body_bytes
    require(len(payload) == ((offset + 4095) & ~4095) and
            payload[offset:] == b"\xff" * (len(payload) - offset), "Unexpected final padding/extra regions")
    return result


def audit(image_path, stock_path, layout_path):
    layout = json.loads(layout_path.read_text())
    require(layout["schema_version"] == 1 and layout["stock_sha256"] == STOCK_SHA256,
            "Unsupported layout/reference identity")
    stock = stock_path.read_bytes()
    require(len(stock) == layout["stock_bytes"] and hashlib.sha256(stock).hexdigest() == STOCK_SHA256,
            "Stock reference is not the verified PLAF103 1.1.15 image")
    reference_payload = container(stock)
    offsets = [int(value, 16) for value in layout["ota_bank_offsets"]]
    require(len(offsets) == 2 and offsets[0] < offsets[1], "Invalid bank offsets")
    mapped_base = int(layout["flash_mapping_base"], 16)
    table_file_offset = 0x17040 + int(layout["bank_table_runtime_address"], 16) - 0x0E000020
    require(struct.unpack_from("<2I", stock, table_file_offset) == tuple(mapped_base + x for x in offsets),
            "Layout bank offsets do not match the verified stock table")
    require(layout["candidate_payload_cap_bytes"] == len(reference_payload), "Unsupported candidate size cap")
    require(image_path.stat().st_size <= len(stock), "Candidate exceeds the known stock reference footprint")
    data = image_path.read_bytes()
    payload = container(data)
    require(len(payload) <= len(reference_payload), "Candidate exceeds the known stock reference footprint")
    require(len(payload) <= offsets[1] - offsets[0], "Candidate crosses the next bank start")
    require(layout["sector_bytes"] == 4096 and len(payload) % 4096 == 0, "Unsupported sector alignment")
    image_regions = regions(payload)
    placements = []
    for index, start in enumerate(offsets):
        end = start + len(payload)
        overlaps = [r["name"] for r in layout["known_preserve_ranges"]
                    if start < int(r["end_exclusive"], 16) and int(r["start"], 16) < end]
        require(not overlaps, f"Candidate overlaps known preserved data: {overlaps}")
        placements.append({"bank_index": index, "flash_offset": f"0x{start:08x}",
                           "end_exclusive": f"0x{end:08x}", "erase_bytes": len(payload)})
    return {"schema_version": 1, "image": str(image_path), "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(), "payload_bytes": len(payload),
            "regions": image_regions, "candidate_placements": placements,
            "checks": {"container_and_checksum": True, "reference_table_matches": True,
                       "known_preserve_ranges_untouched": True},
            "boot_acceptance": "unproven", "physical_flash_capacity": "unverified",
            "scope": "Offline structural/known-range audit; not a flash plan or full partition validation"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("--stock-reference", type=Path,
                        default=ROOT / "private/firmware/PLAF103_OTA_010115.bin")
    parser.add_argument("--layout", type=Path, default=ROOT / "src/firmware/devices/plaf103/layout.json")
    args = parser.parse_args()
    try:
        print(json.dumps(audit(args.image, args.stock_reference, args.layout), indent=2))
    except (OSError, ValueError, KeyError, TypeError, struct.error) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
