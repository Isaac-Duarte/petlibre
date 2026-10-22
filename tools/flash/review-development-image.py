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


B = module("review_development_bank", ROOT / "tools/flash/development-bank.py")
require, digest = B.require, B.digest
STOCK = ROOT / "private/firmware/PLAF103_OTA_010115.bin"


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
    payload = B.payload(image)
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
    layout = B.L.audit(build / ota_path, STOCK, ROOT / "firmware/plaf103-layout.json")
    mapper = module("development_factory_preflight", ROOT / "tools/flash/prepare-firstboot.py")
    baseline = B.T.pinned_baseline(mapper.FACTORY.FLASH.read_bytes())
    rom = mapper.FACTORY.ROM.read_bytes()
    require(digest(rom) == mapper.FACTORY.BOOT.ROM_SHA256, "Unreviewed factory ROM input")
    mapping = mapper.mapping_cases(baseline, rom, image)
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
        require(digest(image) == B.L.STOCK_SHA256, "Pinned stock image required")
        details = {"stock_sha256": B.L.STOCK_SHA256}
    else:
        require(build.resolve().is_relative_to((ROOT / "private").resolve()), "Private build required")
        image, manifest, details = review(build)
    data = B.payload(image)
    result = {"schema_version": 1, "kind": "plaf103_development_image_review", "image_kind": mode,
              "ota_sha256": digest(image), "payload_sha256": digest(data), "payload_bytes": len(data),
              "manifest_sha256": digest(manifest) if manifest else None, "review": details,
              "reviewer_sha256": digest(Path(__file__).read_bytes()), "application_boot_verified": False}
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name, raw in [("image.bin", image), ("review.json", B.canonical(result) + b"\n")]:
        with (output / name).open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
    if manifest is not None:
        with (output / "build-manifest.json").open("xb") as stream:
            stream.write(manifest)
            stream.flush()
            os.fsync(stream.fileno())
    B.C.sync_directory(output)
    B.C.sync_directory(output.parent)
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
