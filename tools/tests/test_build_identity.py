"""Build identities bind source snapshots without bootstrapping dependencies."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "firmware/tools"))
SPEC = importlib.util.spec_from_file_location("firmware_build_identity", ROOT / "firmware/tools/build.py")
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)
sys.path.pop(0)


class BuildIdentityTests(unittest.TestCase):
    def test_identity_is_order_independent_and_binds_profile_and_content(self):
        first = {ROOT / "Cargo.toml": "a" * 64, ROOT / "build.rs": "b" * 64}
        identity = BUILD.source_identity("plaf103-firstboot", first)
        self.assertRegex(identity, r"^[0-9a-f]{64}$")
        self.assertEqual(identity, BUILD.source_identity("plaf103-firstboot", dict(reversed(list(first.items())))))
        self.assertNotEqual(identity, BUILD.source_identity("bringup", first))
        self.assertNotEqual(identity, BUILD.source_identity("plaf103-firstboot", first | {ROOT / "build.rs": "c" * 64}))

    def test_offline_identity_command_never_bootstraps(self):
        output = io.StringIO()
        with patch.object(sys, "argv", ["build.py", "source-id"]), \
                patch.object(BUILD, "bootstrap", side_effect=AssertionError("must stay offline")), \
                contextlib.redirect_stdout(output):
            BUILD.main()
        self.assertTrue(re.fullmatch(r"[0-9a-f]{64}\n", output.getvalue()))


GIT = ["git", "-c", "user.name=wellfed-test", "-c", "user.email=test@example.invalid",
       "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"]


def git(directory, *args):
    return subprocess.run([*GIT, *args], cwd=directory, check=True, capture_output=True)


class SourceInputTests(unittest.TestCase):
    def test_rust_sources_follow_git_and_exclude_target_and_host_crates(self):
        with tempfile.TemporaryDirectory() as temporary:
            # The checkout itself lives below a directory named `target`.
            root = Path(temporary) / "target" / "checkout"
            rust = root / "crates"
            for name in ("wellfed-core/src/lib.rs", "wellfed-core/src/new.rs", "wellfed-core/src/lib.rs.swp",
                         "wellfed-core/target/out.rs", "wellfed-core/src/target/mod.rs",
                         "wellfed-local/src/main.rs"):
                (rust / name).parent.mkdir(parents=True, exist_ok=True)
                (rust / name).write_text(name)
            (root / ".gitignore").write_text("*.swp\n")
            git(root, "init", "-q")
            git(root, "add", ".gitignore", "crates/wellfed-core/src/lib.rs")
            with patch.object(BUILD, "ROOT", root), patch.object(BUILD, "RUST", rust):
                names = [str(p.relative_to(rust)) for p in BUILD.rust_sources()]
                self.assertEqual(names, ["wellfed-core/src/lib.rs", "wellfed-core/src/new.rs"])
                (rust / "wellfed-core/src/new.rs").unlink()
                (rust / "wellfed-core/src/lib.rs").unlink()
                with self.assertRaises(RuntimeError):
                    BUILD.rust_sources()

    def test_source_identity_requires_git_checkout(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "crates").mkdir(parents=True)
            with patch.object(BUILD, "ROOT", root), patch.object(BUILD, "RUST", root / "crates"), \
                    patch.dict(os.environ, {"GIT_CEILING_DIRECTORIES": str(root.parent)}):
                with self.assertRaisesRegex(RuntimeError, "Git checkout"):
                    BUILD.rust_sources()

    def test_inputs_bind_toolchain_file_pins(self):
        self.assertIn(BUILD.TOOL_LOCK, BUILD.source_inputs("baseline"))
        self.assertIn(BUILD.TOOL_LOCK, BUILD.source_inputs("plaf103-firstboot"))


class SdkWorktreeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.sdk = Path(self.temporary.name) / "sdk"
        tracked = [*BUILD.VENDOR_MODE_ONLY, *BUILD.VENDOR_REBUILT, *BUILD.managed_sdk_paths("plaf103-firstboot"),
                   "component/other.c"]
        for name in tracked:
            (self.sdk / name).parent.mkdir(parents=True, exist_ok=True)
            (self.sdk / name).write_text("pinned " + name)
        git(self.sdk, "init", "-q")
        git(self.sdk, "add", ".")
        git(self.sdk, "commit", "-q", "-m", "pinned")

    def tearDown(self):
        self.temporary.cleanup()

    def changes(self, profile="plaf103-firstboot"):
        return BUILD.unexpected_sdk_changes(self.sdk, profile)

    def test_vendor_outputs_and_profile_overrides_are_allowed(self):
        for name in BUILD.VENDOR_MODE_ONLY:
            (self.sdk / name).chmod(0o755)
        for name in BUILD.VENDOR_REBUILT | BUILD.managed_sdk_paths("plaf103-firstboot"):
            (self.sdk / name).write_text("regenerated")
        for name in BUILD.VENDOR_UNTRACKED:
            (self.sdk / name).parent.mkdir(parents=True, exist_ok=True)
            (self.sdk / name).write_text("vendor output")
        self.assertEqual(self.changes(), [])
        BUILD.check_sdk_worktree(self.sdk, "plaf103-firstboot")

    def test_unexpected_changes_are_named_and_refused(self):
        mode_only = sorted(BUILD.VENDOR_MODE_ONLY)[0]
        (self.sdk / mode_only).chmod(0o755)
        (self.sdk / mode_only).write_text("changed helper")
        (self.sdk / "component/other.c").write_text("local edit")
        (self.sdk / "component/new.c").write_text("stray source")
        main = "project/realtek_amebaD_va0_example/src/src_lp/main.c"
        (self.sdk / main).write_text("first-boot main")
        self.assertEqual(self.changes(), sorted([f"M {mode_only}", "M component/other.c", "?? component/new.c"]))
        # A first-boot main is only managed by the first-boot profile.
        self.assertIn(f"M {main}", self.changes("plaf103"))
        with self.assertRaisesRegex(RuntimeError, "component/other.c"):
            BUILD.check_sdk_worktree(self.sdk, "plaf103-firstboot")

    def test_staged_and_deleted_entries_are_refused(self):
        managed = sorted(BUILD.managed_sdk_paths("plaf103-firstboot"))
        (self.sdk / managed[0]).write_text("staged")
        git(self.sdk, "add", managed[0])
        (self.sdk / managed[1]).unlink()
        self.assertEqual(self.changes(), sorted([f"M {managed[0]}", f"D {managed[1]}"]))


class ToolchainTests(unittest.TestCase):
    def archive(self, directory, members):
        path = directory / "toolchain.tar.bz2"
        with tarfile.open(path, "w:bz2") as bundle:
            for name, data in members.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                info.mode = 0o755
                bundle.addfile(info, io.BytesIO(data))
        return path

    def test_extraction_is_verified_and_atomic(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            destination = base / "toolchains"
            destination.mkdir()
            pins = {"bin/arm-none-eabi-gcc": hashlib.sha256(b"gcc").hexdigest()}
            archive = self.archive(base, {"asdk-6.4.1/linux/newlib/bin/arm-none-eabi-gcc": b"gcc"})
            with patch.object(BUILD, "toolchain_lock", return_value=pins):
                BUILD.extract_toolchain(archive, destination, "asdk-6.4.1/linux/newlib")
                self.assertEqual(sorted(p.name for p in destination.iterdir()), ["asdk-6.4.1"])
                toolchain = destination / "asdk-6.4.1/linux/newlib"
                BUILD.verify_toolchain(toolchain)
                # An existing (possibly partial) tree is never extracted over.
                with self.assertRaisesRegex(RuntimeError, "Partial"):
                    BUILD.extract_toolchain(archive, destination, "asdk-6.4.1/linux/newlib")
                (toolchain / "bin/arm-none-eabi-gcc").write_bytes(b"modified")
                with self.assertRaisesRegex(RuntimeError, "changed"):
                    BUILD.verify_toolchain(toolchain)
                (toolchain / "bin/arm-none-eabi-gcc").unlink()
                with self.assertRaisesRegex(RuntimeError, "missing"):
                    BUILD.verify_toolchain(toolchain)

    def test_wrong_or_escaping_archives_leave_no_tree(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            destination = base / "toolchains"
            destination.mkdir()
            pins = {"bin/arm-none-eabi-gcc": hashlib.sha256(b"gcc").hexdigest()}
            for members in ({"asdk-6.4.1/linux/newlib/bin/arm-none-eabi-gcc": b"other"},
                            {"asdk-6.4.1/linux/newlib/bin/arm-none-eabi-gcc": b"gcc", "elsewhere/x": b"x"}):
                archive = self.archive(base, members)
                with self.subTest(members=sorted(members)), patch.object(BUILD, "toolchain_lock", return_value=pins):
                    with self.assertRaises(RuntimeError):
                        BUILD.extract_toolchain(archive, destination, "asdk-6.4.1/linux/newlib")
                    self.assertEqual(list(destination.iterdir()), [])

    def test_pins_must_describe_the_sdk_archive(self):
        self.assertTrue(BUILD.toolchain_lock())
        with patch.dict(BUILD.LOCK["toolchain"], {"sha256": "0" * 64}):
            with self.assertRaises(RuntimeError):
                BUILD.toolchain_lock()


class RustEnvironmentTests(unittest.TestCase):
    def test_output_affecting_overrides_are_refused(self):
        clean = {k: v for k, v in os.environ.items()
                 if k not in BUILD.RUST_ENVIRONMENT and not k.startswith(("CARGO_PROFILE_", "CARGO_UNSTABLE_", "CARGO_TARGET_"))}
        with patch.object(BUILD, "cargo_config_files", return_value=[]):
            with patch.dict(os.environ, clean, clear=True):
                self.assertNotIn("RUSTFLAGS", BUILD.rust_environment())
            for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTUP_TOOLCHAIN",
                         "CARGO_PROFILE_FIRMWARE_OPT_LEVEL", "CARGO_TARGET_THUMBV8M_MAIN_NONE_EABIHF_RUSTFLAGS"):
                with self.subTest(name=name), patch.dict(os.environ, {**clean, name: "x"}, clear=True):
                    with self.assertRaisesRegex(RuntimeError, name):
                        BUILD.rust_environment()
            with patch.dict(os.environ, {**clean, "CARGO_BUILD_TARGET_DIR": "/tmp/x",
                                         "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS": "-Clink-arg=x"}, clear=True):
                BUILD.rust_environment()

    def test_cargo_configuration_overrides(self):
        allowed = ('# [build]\n# rustc-wrapper = "sccache"\n[target.x86_64-unknown-linux-gnu]\n'
                   'linker = "clang"\nrustflags = ["-Clink-arg=--ld-path=mold"]\n[build]\njobs = 4\n'
                   "[target.'thumbv8m.main-none-eabihf']\nrunner = \"probe\"\n")
        self.assertEqual(BUILD.cargo_config_overrides(allowed), [])
        for text, key in [('[build]\nrustflags = ["-Copt-level=0"]\n', "build.rustflags"),
                          ('build.rustc-wrapper = "sccache"\n', "build.rustc-wrapper"),
                          ('build = { rustflags = ["x"] }\n', "build"),
                          ('[profile.firmware]\nopt-level = 0\n', "profile.firmware.opt-level"),
                          ("[target.'thumbv8m.base-none-eabi']\nrustflags = [\n  \"x\",\n]\n",
                           "target.thumbv8m.base-none-eabi.rustflags"),
                          ("[target.'cfg(target_os = \"none\")']\nrustflags = [\"x\"]\n",
                           'target.cfg(target_os = "none").rustflags'),
                          ('[unstable]\nbuild-std = ["core"]\n', "unstable.build-std")]:
            with self.subTest(key=key):
                self.assertEqual(BUILD.cargo_config_overrides(text), [key])

    def test_configuration_files_are_refused_with_their_path(self):
        clean = {k: v for k, v in os.environ.items()
                 if k not in BUILD.RUST_ENVIRONMENT and not k.startswith(("CARGO_PROFILE_", "CARGO_UNSTABLE_", "CARGO_TARGET_"))}
        with tempfile.TemporaryDirectory() as temporary:
            config = Path(temporary) / "config.toml"
            config.write_text('[profile.firmware]\nlto = false\n')
            with patch.dict(os.environ, clean, clear=True), \
                    patch.object(BUILD, "cargo_config_files", return_value=[config]):
                with self.assertRaisesRegex(RuntimeError, "profile.firmware.lto"):
                    BUILD.rust_environment()


class EvidenceFreshnessTests(unittest.TestCase):
    def test_stale_or_missing_build_manifests_are_unavailable(self):
        from support import current_build
        with tempfile.TemporaryDirectory() as temporary:
            build = Path(temporary) / "baseline"
            build.mkdir()
            self.assertFalse(current_build(build)[0])
            inputs = {str(p.relative_to(ROOT)): BUILD.digest(p) for p in BUILD.source_inputs("baseline")}
            (build / "manifest.json").write_text(json.dumps({"profile": "baseline", "inputs": inputs}))
            self.assertEqual(current_build(build), (True, ""))
            changed = dict(inputs, **{"firmware/sdk.lock.json": "0" * 64})
            (build / "manifest.json").write_text(json.dumps({"profile": "baseline", "inputs": changed}))
            available, reason = current_build(build)
            self.assertFalse(available)
            self.assertIn("Stale baseline build", reason)
            self.assertIn("firmware/sdk.lock.json", reason)
