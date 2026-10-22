#!/usr/bin/env python3
"""Freeze a reviewed PLAF103 development image offline, without serial access."""
import argparse
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from importlib.util import module_from_spec, spec_from_file_location

ROOT = Path(__file__).resolve().parents[2]


def module(name, path):
    spec = spec_from_file_location(name, path)
    value = module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


C = module("review_coordinator", ROOT / "tools/flash/recovery-transaction.py")
L = module("review_layout", ROOT / "tools/audit/audit-ota-layout.py")
require, digest, canonical = C.require, C.digest, C.canonical
BANK_BYTES = C.SECTORS.END - C.SECTORS.START
SECTOR = C.SECTORS.SECTOR
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"


FACTORY = None


def bank_payload(image):
    """The application payload, sector aligned within the reviewed bank footprint."""
    require(type(image) is bytes, "Immutable complete OTA bytes required")
    data = L.container(image)
    require(0 < len(data) <= BANK_BYTES and len(data) % SECTOR == 0,
            "Development image exceeds aligned bank-1 footprint")
    L.regions(data)  # Both cores, exact addresses, headers and padding.
    return data


def factory():
    """The factory-mapper emulation, loaded only when a review needs it (it needs unicorn)."""
    global FACTORY
    if FACTORY is None:
        FACTORY = module("review_factory_mapping", ROOT / "tools/audit/audit-factory-mapping.py")
    return FACTORY


def mapping_cases(current, rom, image):
    """Run the actual factory mapper on each bank holding `image` alone; never sent."""
    mapper = factory()
    payload = L.container(image)
    regions = L.regions(payload)
    split = regions[2]["payload_header_offset"]
    cases = []
    for bank, start in enumerate(mapper.BANKS):
        fixture = bytearray(current)
        fixture[start:start + len(payload)] = payload
        # This fixture deliberately selects just this bank.
        other = mapper.BANKS[1 - bank]
        fixture[other:other + 8] = b"\x00" * 8
        checked = mapper.execute(fixture, rom, bank)
        require(checked["stop"] == hex(mapper.EXIT) and checked["result"] == 1,
                "Actual factory instructions reject diagnostic headers")
        require(len(checked["configs"]) == 4 and checked["commands"] == [[0, 1], [1, 1]],
                "Unexpected factory mapper sequence")
        for index, physical, length in ((0, start, split), (1, start + split, len(payload) - split)):
            final = checked["configs"][index * 2 + 1]
            virtual = (0x0c000000, 0x0e000000)[index]
            require(final["start"] == virtual and final["end"] == virtual + length - 1 and
                    final["minus"] == 1 and final["offset"] == virtual - 0x08000000 - physical,
                    "Factory mapping does not cover packaged diagnostic")
        chosen = mapper.execute(fixture, rom)
        require(chosen["stop"] == "0x80004a0" and chosen["result"] == bank + 1,
                "Single-valid diagnostic bank selection differs")
        cases.append({"bank": bank, "bank_check": checked, "selection": chosen})
    return cases


def local(parent, name):
    path = (parent / name).resolve()
    require(path.is_relative_to(parent.resolve()) and path.is_file(), "Manifest file escapes its root")
    return path


def review(build):
    build = Path(build).resolve()
    sys.path.insert(0, str(ROOT / "firmware/tools"))
    builder = module("development_builder", ROOT / "firmware/tools/build.py")
    raw = (build / "manifest.json").read_bytes()
    m = json.loads(raw)
    require(m["profile"] == "plaf103-firstboot", "Only the reviewed UART startup profile is supported")
    require(m["sdk_revision"] == builder.LOCK["sdk"]["revision"] and
            m["toolchain_sha256"] == builder.LOCK["toolchain"]["sha256"], "Unexpected SDK/toolchain")
    require(builder.revision(build / "sdk") == m["sdk_revision"], "SDK worktree revision differs")
    expected_inputs = {str(p.relative_to(ROOT)): builder.digest(p)
                       for p in builder.source_inputs(m["profile"])}
    require(m["inputs"] == expected_inputs, "Build inputs are stale or incomplete; rebuild before review")
    for name, expected in m["inputs"].items():
        require(digest(local(ROOT, name).read_bytes()) == expected, "Build input differs")
    for name, expected in m["sdk_overrides"].items():
        require(digest(local(build / "sdk", name).read_bytes()) == expected, "SDK overlay differs")
    names = [Path(a["path"]).name for a in m["artifacts"]]
    require(sorted(names) == sorted(["km0_image2_all.bin", "km4_image2_all.bin",
                                     "km0_km4_image2.bin", "OTA_All.bin"]), "Incomplete packaged artifacts")
    files = {}
    for artifact in m["artifacts"]:
        data = local(build, artifact["path"]).read_bytes()
        require(type(artifact["bytes"]) is int and len(data) == artifact["bytes"] and
                digest(data) == artifact["sha256"], "Build artifact differs")
        files[Path(artifact["path"]).name] = data
    image = files["OTA_All.bin"]
    payload = bank_payload(image)
    require(payload == files["km0_km4_image2.bin"] and
            payload == files["km0_image2_all.bin"] + files["km4_image2_all.bin"],
            "Both core packages must exactly compose the OTA payload")
    objects = {}
    for entry in m["compiled_archives"]:
        require(entry["core"] not in objects, "Duplicate core archive")
        path = local(build, entry["path"])
        require(digest(path.read_bytes()) == entry["sha256"], "Compiled archive differs")
        objects[entry["core"]] = path
    require(set(objects) == {"KM0", "KM4"}, "Both compiled cores required")
    tools = ROOT / "private/toolchains" / builder.LOCK["toolchain"]["directory"] / "bin"
    sdk = build / "sdk"
    mains = builder.firstboot.audit(sdk, builder.PROJECT, tools)
    parking = builder.rampark.audit(sdk, builder.PROJECT, tools, objects)
    hooks = builder.sdkhooks.audit(sdk, builder.PROJECT, tools, objects)
    require(mains == m["firstboot_startup_audit"] and parking == m["ram_park_audit"] and
            hooks == m["sdk_hook_audit"], "Linked-code audits differ from build manifest")
    pinmap = builder.pinmap.audit_linked_table(image,
        int(m["km0_pinmap_audit"]["runtime_address"], 16), STOCK.read_bytes())
    require(pinmap == m["km0_pinmap_audit"], "Linked pad table differs")
    ota_path = next(a["path"] for a in m["artifacts"] if Path(a["path"]).name == "OTA_All.bin")
    layout = L.audit(build / ota_path, STOCK, ROOT / "firmware/plaf103-layout.json")
    mapper = factory()
    baseline = C.SECTORS.pinned_baseline(mapper.FLASH.read_bytes())
    rom = mapper.ROM.read_bytes()
    require(digest(rom) == mapper.BOOT.ROM_SHA256, "Unreviewed factory ROM input")
    mapping = mapping_cases(baseline, rom, image)
    # Detect edits during potentially lengthy disassembly/emulation.
    require((build / "manifest.json").read_bytes() == raw and
            {str(p.relative_to(ROOT)): builder.digest(p)
             for p in builder.source_inputs(m["profile"])} == expected_inputs,
            "Build changed during review")
    for artifact in m["artifacts"]:
        require(digest(local(build, artifact["path"]).read_bytes()) == artifact["sha256"],
                "Artifact changed during review")
    return image, raw, {"startup_mains": mains["mains"], "layout": layout,
                        "pinmap": pinmap, "factory_mapping": mapping,
                        "scope": "Package, linked code, stock pulls and modeled mapping; application behavior requires review"}


def freeze(mode, build, output):
    require(output.resolve().is_relative_to((ROOT / "private").resolve()), "New private output required")
    manifest = None
    if mode == "stock":
        image = STOCK.read_bytes()
        require(digest(image) == L.STOCK_SHA256, "Pinned stock image required")
        details = {"stock_sha256": L.STOCK_SHA256}
    else:
        require(build.resolve().is_relative_to((ROOT / "private").resolve()), "Private build required")
        image, manifest, details = review(build)
    data = bank_payload(image)
    result = {"schema_version": 1, "kind": "plaf103_development_image_review", "image_kind": mode,
              "ota_sha256": digest(image), "payload_sha256": digest(data), "payload_bytes": len(data),
              "manifest_sha256": digest(manifest) if manifest else None, "review": details,
              "reviewer_sha256": digest(Path(__file__).read_bytes()), "application_boot_verified": False}
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name, raw in [("image.bin", image), ("review.json", canonical(result) + b"\n")]:
        with (output / name).open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
    if manifest is not None:
        with (output / "build-manifest.json").open("xb") as stream:
            stream.write(manifest)
            stream.flush()
            os.fsync(stream.fileno())
    C.sync_directory(output)
    C.sync_directory(output.parent)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("stock", "build"))
    parser.add_argument("--build", type=Path, default=ROOT / "private/firmware-build/plaf103-firstboot")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    result = freeze(args.mode, args.build, args.output)
    print(json.dumps({k: result[k] for k in ("image_kind", "ota_sha256", "payload_bytes")}))


if __name__ == "__main__":
    main()
