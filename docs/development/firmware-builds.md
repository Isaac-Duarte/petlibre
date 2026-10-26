# SDK-backed firmware builds

Host tests and cross-compilation checks need no vendor SDK. Full images use the pinned Realtek AmebaD SDK and ASDK compiler specified by [sdk.lock.json](../../src/firmware/vendor/sdk.lock.json). The SDK baseline is a family-level reference, not a source match for stock `sdk-amebad_v6.2C-RC`.

## Prerequisites and explicit setup

The existing wrapper supports **Linux x86_64**. It needs Git, Make, Bash, tar (with bzip2), curl, standard GNU tools, the pinned Rust toolchain, libclang for bindgen, and the 32-bit loader/runtime needed by Realtek's i686 compiler. Install platform prerequisites yourself; the wrapper installs no system packages.

The builder is `wellfed-build` (`src/tools/wellfed-build`); the make targets build it and run it as its own executable, because `cargo run` sets `RUSTUP_TOOLCHAIN`, which a firmware build refuses.

## Devices, firmwares and profiles

A build is a **profile** of a **firmware** on a **device**:

```sh
wellfed-build build --profile firstboot [--device plaf103] [--firmware qa] [--variant fail]
make -C src/firmware/vendor firstboot DEVICE=plaf103 FIRMWARE=trial VARIANT=fail
wellfed-build list    # the devices, and each firmware's devices and variants, as JSON
```

- **Device:** `src/firmware/devices/<name>/device.json` names the board's platform (the chip family) and the files that platform stages: the application entry and first-boot mains, plus the verified stock image and [layout](../../src/firmware/devices/plaf103/layout.json) the `device` and `firstboot` profiles derive their banks and pad table from. `--device` defaults to the only device; with several, a build must name one.
- **Firmware:** a crate under `src/firmware/wellfed/` ([guide](../../src/firmware/wellfed/README.md)). Its `[package.metadata.wellfed]` table declares the devices it builds for, the startup policies of its normal first-boot image, and its opt-in image variants. A variant is the Cargo feature of the same name, and its policies replace the normal image's. A new firmware or variant needs no builder change.
- **Platform:** code, not configuration (`wellfed-build`'s `src/platform/`). Its SDK overlays anchor on exact text in the pinned SDK and its audits read disassembly, so both fail closed. AmebaD is the only platform; another chip is a new variant of `Platform` and a module beside `amebad`.

```sh
make -C src/firmware/vendor bootstrap
make -C src/firmware/vendor baseline
make -C src/firmware/vendor bringup
```

`bootstrap` explicitly fetches the SDK revision and downloads/checksums the compiler archive. The archive digest was computed from the official release download; it is not an independently supplied upstream signature. Existing unexpected SDK revisions are refused. The compiler is extracted into a temporary directory (only entries under its pinned directory; links must stay inside it, and special files are refused), checked, then renamed into place; an existing partial tree is refused rather than reused. Every run verifies the pinned compiler/binutils file hashes in [toolchain.lock.json](../../src/firmware/vendor/probe/toolchain.lock.json). Check targets never bootstrap implicitly.

A reused SDK worktree is refused, with the offending paths named, when `git status` shows anything outside the profile's own overlays and the exact vendor-recipe outputs listed in `wellfed-build`'s `sdk.rs` (executable-bit changes with unchanged content, the rebuilt `lib_user.a`, toolchain links and two KM0 loader outputs). Nothing is cleaned automatically; review, then restore or recreate the worktree. Files ignored by the SDK's own rules are not checked.

Firmware builds refuse output-affecting Rust overrides instead of silently dropping them: `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, `RUSTC*`, `RUSTUP_TOOLCHAIN`, `CARGO_PROFILE_*`, `CARGO_UNSTABLE_*`, incremental settings, firmware-target `CARGO_TARGET_*_RUSTFLAGS`, and the equivalent `build`/`profile`/`unstable`/firmware-target keys in applicable Cargo configuration files. Host-only target settings such as a host linker are allowed. The manifest records the `rustc -vV` that cargo resolves from the repository, which must match `rust-toolchain.toml`.

## Profiles

Each profile adds stages to the one before.

| Profile | Behavior and inputs |
| --- | --- |
| `baseline` | Unchanged SDK example, for build comparison |
| `bringup` | SDK example with the Rust application, STOP/off initialization, and bounded indicator entry hook |
| `device` | The application with the device's stock-derived bank addresses and audited KM0 pin pulls; requires the device's verified stock OTA |
| `firstboot` | Minimal UART-only SDK mains plus the application, Wi-Fi, transport and updates; same stock-reference requirements. The image the bench reviews and installs; only this profile takes a variant |

```sh
make -C src/firmware/vendor device
make -C src/firmware/vendor firstboot
```

For the PLAF103, those two profiles require a contributor-supplied exact stock reference at `private/firmware/PLAF103_OTA_010115.bin` (named in its `device.json`); see [fixtures](fixtures.md). Builds proceed KM0 before KM4 and retain input hashes, artifact hashes, logs, and successful manifests. Bindgen gets SDK/newlib paths from the wrapper through `WELLFED_SDK_DIR` and `WELLFED_NEWLIB_INCLUDE`.

| Ignored location | Contents |
| --- | --- |
| `private/upstream/ameba-rtos-d/` | Pinned upstream checkout |
| `private/toolchains/` | Downloaded archive and extracted compiler |
| `private/firmware-build/<device>/<profile>/` | Generated SDK worktree, Rust archives, logs, and manifest |

Author changes in tracked source, not generated SDK worktrees. Profiles stage tracked files and apply audited overlays. Vendor timestamps mean output is not guaranteed byte-for-byte reproducible. A rebuilt UART diagnostic will generally differ from the exact artifact accepted by the retained writer; do not update its hash to bypass review.

Builds package application images and audit symbols/layout/pin pulls/flash hooks where applicable. They do not flash, actuate a motor, validate a physical board, or authorize replacing the factory loader. Upstream example startup can still touch hardware before the diagnostic. See [architecture](../firmware/README.md), [third-party notices](../../THIRD_PARTY.md), and [recovery restrictions](../devices/plaf103/recovery.md).

## Source identity and physical startup behavior

`cargo run -q -p wellfed-build -- source-id` computes the current firstboot-profile source ID offline without bootstrapping the SDK. `check-cross` supplies that ID to the SDK-independent KM0 port compile. `source-id --image` (with `--variant`, if any) prints instead the identity a build would embed, which also binds the variant and the update key. Every identity binds the device and profile names. `wellfed-bench` uses it to reuse a review of the same sources. A compile-only check does not produce a reviewed installable image. SDK builds freeze the input map, embed its real identity, and record clock/output/LED/routing/shared-pad policies for host verification. Rust inputs are the Git-tracked and untracked, non-ignored files of the SDK crates under `src/sdk/`, the product crates under `src/product/` they link, and the selected firmware's crate (not `target` directories there, nor other firmwares or host tools), the device's `device.json` and the files it names that the profile stages, plus the builder's own `src/` and the two `wellfed-flash` modules its layout audit shares, so a changed recipe or audit is a new identity; its fixture tests are not. Ignored editor or build files and the checkout's location do not change the ID; `source-id` therefore needs a Git checkout. `wellfed-build inputs --profile PROFILE` prints the current input map, and `wellfed-build audit --build DIR` re-runs a finished build's audits as JSON; `wellfed-build review` checks a build against both before freezing it. The builder's fixture tests (`cargo test -p wellfed-build -- --ignored`, in `make check-fixtures`) fail when an ignored build no longer matches the current sources.

The diagnostic now configures STOP/off GPIO, restores shared-pad supply startup in the minimal KM0 main, and pulses the four indicators. An unopened feeding backend no longer means no physical GPIO changes. See [startup behavior and outstanding readback faults](../firmware/startup-diagnostics.md). Build commands themselves still do not access hardware. Use [development image review](uart-development.md) for a source-bound target; copied historical manifests may be stale after porting.

## Update signing

The replacement firmware installs an update only if its manifest is signed by the key the build embeds. Both commands run offline and never contact a device:

```sh
./target/release/wellfed-build keygen
./target/release/wellfed-build sign --review private/verification/<review> --version 2 \
  --output private/<name>.manifest
```

- **`keygen`:** writes a new Ed25519 key to `private/ota-key.json` (mode 600), marked `"scope": "bench"`, and prints the public key that first-boot builds embed (it joins their identity). It refuses to replace an existing key.
- **`sign`:** signs only a directory under `private/` that `wellfed-build review` froze, verified exactly as `wellfed-flash` verifies a review before installing it. It refuses a directory without `review.json`, an `image.bin` or payload that differs from the review, a `build-manifest.json` that differs from its `manifest_sha256`, a stock review of anything but the pinned stock image, a build that is not the first-boot profile of the locked SDK and toolchain or whose manifest's `OTA_All.bin` is not this image, and missing startup, pad-table or mapping audits. It also requires the image to carry the reviewed startup identity. The build ID comes from the verified manifest (for the stock image, the review's payload hash), never from the command line. An image whose build declares a startup motion policy signs only with `--attended-motion POLICY` naming that exact policy. The default is the version 2 manifest the device API takes (`wellfed_core::ota::offer`: it names the bench key's id and the image's kind); `--format mqtt` writes version 1, byte for byte what the former `wellfed-local ota-sign` wrote (a test pins it), for a build still on MQTT. `--version` must exceed the feeder's floor (the highest version it confirmed), or come with a downgrade statement.
- **`downgrade --manifest M --device HEX --floor N`:** a statement letting the feeder whose identity key is `HEX` install the version 2 manifest `M` while its floor is `N`: a version at or below the floor, or any stock image, needs one.
- **`revoke --key-id HEX`** and **`add-key --public HEX`:** key statements that retire an update key id for good, or fill the firmware's release slot ([OTA design](../firmware/ota-design.md#keys-and-anti-rollback)). Only a key built into the firmware signs them; `keygen` prints the bench key's id.

`wellfed-bench ota` links the same library and signs with `private/ota-key.json`.

**The key is the bench key.** It lives in a checkout's `private/` and signs development images for the bench's own units only; a key file without a `scope` field predates it and is read as the bench key, and the signer loads no other scope. A public release needs an offline release key first, which never sits in a checkout and which this tool does not create.
