# Firmware builds

This guide builds a full firmware image with the vendor SDK, and signs one as an update. It is for contributors who already pass `make check` ([getting started](README.md)). Building never flashes hardware.

Host tests and cross-compilation checks need no vendor SDK. Full images use the pinned Realtek AmebaD SDK and ASDK compiler named in [sdk.lock.json](../../src/firmware/vendor/sdk.lock.json). That SDK baseline is a family-level reference, not a source match for stock's `sdk-amebad_v6.2C-RC`.

## Prerequisites

The build wrapper supports **Linux x86_64**. It needs:

- Git, Make, Bash, tar with bzip2, curl and the standard GNU tools;
- the pinned Rust toolchain;
- libclang, for bindgen;
- the 32-bit loader and runtime that Realtek's i686 compiler needs.

Install these yourself. The wrapper installs no system packages.

The builder is `wellfed-build` (`src/tools/wellfed-build`). The make targets build it and run it as its own executable. They don't use `cargo run`, because it sets `RUSTUP_TOOLCHAIN`, which a firmware build refuses.

## Devices, firmwares and profiles

A build is a **profile** of a **firmware** on a **device**:

```sh
wellfed-build build --profile firstboot [--device plaf103] [--firmware qa] [--variant fail]
make -C src/firmware/vendor firstboot DEVICE=plaf103 FIRMWARE=bringup VARIANT=fail
wellfed-build list    # the devices, and each firmware's devices and variants, as JSON
```

- **Device:** `src/firmware/devices/<name>/device.json` names the board's platform (the chip family) and the files that platform stages. Those are the application entry and first-boot mains, plus the verified stock image and [layout](../../src/firmware/devices/plaf103/layout.json). The `device` and `firstboot` profiles derive their banks and pad table from that layout. `--device` defaults to the only device; with several, a build must name one.
- **Firmware:** a crate under `src/firmware/wellfed/` ([firmware guide](../../src/firmware/wellfed/README.md)). Its `[package.metadata.wellfed]` table declares the devices it builds for, the startup policies of its normal first-boot image, and its opt-in image variants. A variant is the Cargo feature of the same name, and its policies replace the normal image's. A new firmware or variant needs no builder change.
- **Platform:** code, not configuration (`wellfed-build`'s `src/platform/`). Its SDK overlays anchor on exact text in the pinned SDK, and its audits read disassembly, so both fail closed. AmebaD is the only platform. Another chip would be a new variant of `Platform` and a module beside `amebad`.

### Profiles

Each profile adds stages to the one before.

| Profile | What it builds |
| --- | --- |
| `baseline` | The unchanged SDK example, for build comparison |
| `bringup` | The SDK example with the Rust application, STOP/off initialization, and a bounded indicator entry hook |
| `device` | The application with the device's stock-derived bank addresses and audited KM0 pin pulls. Needs the device's verified stock OTA |
| `firstboot` | Minimal UART-only SDK mains plus the application, Wi-Fi, the device API and updates. Same stock-reference need. This is the image the bench reviews and installs, and the only profile that takes a variant |

## Build an image

1. Fetch the SDK and compiler:

   ```sh
   make -C src/firmware/vendor bootstrap
   ```

   You should see the SDK revision fetched and the compiler archive downloaded and checksummed into `private/`. Check targets never run this step for you.

2. Build the unchanged SDK example and the bring-up profile:

   ```sh
   make -C src/firmware/vendor baseline
   make -C src/firmware/vendor bringup
   ```

3. For the PLAF103's `device` and `firstboot` profiles, place your own exact stock 1.1.15 image at `private/firmware/PLAF103_OTA_010115.bin`. Its `device.json` names that path, and the layout file records its hash ([fixtures](fixtures.md)).

4. Build the device profiles:

   ```sh
   make -C src/firmware/vendor device
   make -C src/firmware/vendor firstboot
   ```

   You should see the build, then its audits, finish without a refusal. The manifest, logs and Rust archives land in `private/firmware-build/<device>/<profile>/`.

5. To freeze the image for installing, continue with [development updates](uart-development.md).

Builds run KM0 before KM4. They keep input hashes, artifact hashes, logs and successful manifests. Bindgen gets the SDK and newlib paths from the wrapper through `WELLFED_SDK_DIR` and `WELLFED_NEWLIB_INCLUDE`.

### Where output goes

| Ignored location | Contents |
| --- | --- |
| `private/upstream/ameba-rtos-d/` | The pinned upstream checkout |
| `private/toolchains/` | The downloaded archive and extracted compiler |
| `private/firmware-build/<device>/<profile>/` | The generated SDK worktree, Rust archives, logs and manifest |

Make your changes in tracked source, never in a generated SDK worktree. Profiles stage tracked files and apply audited overlays.

## What the builder refuses

**Bootstrap.** The compiler archive digest was computed from the official release download. It is not an independently supplied upstream signature.

- An existing SDK checkout at an unexpected revision is refused.
- The compiler is extracted into a temporary directory, then checked and renamed into place. Only entries under its pinned directory are extracted. Links must stay inside it, and special files are refused.
- An existing partial compiler tree is refused rather than reused.
- Every run verifies the pinned compiler and binutils file hashes in [toolchain.lock.json](../../src/firmware/vendor/probe/toolchain.lock.json).

**A dirty SDK worktree.** A reused worktree is refused, with the offending paths named, when `git status` shows anything outside two allowed sets: the profile's own overlays, and the exact vendor-recipe outputs listed in `wellfed-build`'s `sdk.rs`. Those outputs are executable-bit changes with unchanged content, the rebuilt `lib_user.a`, toolchain links and two KM0 loader outputs. Nothing is cleaned automatically: review, then restore or recreate the worktree. Files the SDK's own rules ignore are not checked.

**Rust overrides.** Firmware builds refuse output-affecting overrides instead of silently dropping them:

- `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, `RUSTC*`, `RUSTUP_TOOLCHAIN`, `CARGO_PROFILE_*`, `CARGO_UNSTABLE_*`, incremental settings and firmware-target `CARGO_TARGET_*_RUSTFLAGS`;
- the equivalent `build`, `profile`, `unstable` and firmware-target keys in applicable Cargo configuration files.

Host-only target settings, such as a host linker, are allowed. The manifest records the `rustc -vV` that cargo resolves from the repository, which must match `rust-toolchain.toml`.

## What a build does and doesn't do

Builds package application images and audit symbols, layout, pin pulls and flash hooks where they apply. They do not flash, drive a motor, validate a physical board, or authorize replacing the factory loader.

The images themselves do touch hardware when they run. Upstream example startup can touch hardware before the diagnostic. The diagnostic configures STOP/off GPIO, restores shared-pad supply startup in the minimal KM0 main, and pulses the four indicators ([startup behavior and outstanding readback faults](../firmware/startup-diagnostics.md)).

Vendor timestamps mean output is not guaranteed byte-for-byte reproducible.

> [!WARNING]
> A rebuilt UART diagnostic will generally differ from the exact artifact the retained recovery writer accepts. Never update its hash to get past review ([recovery restrictions](../devices/plaf103/recovery.md)).

## Source identity

Every build carries a source identity that binds exactly what it was built from.

- `cargo run -q -p wellfed-build -- source-id` computes the current firstboot-profile source ID offline, without bootstrapping the SDK. `check-cross` supplies that ID to the SDK-independent KM0 port compile.
- `source-id --image` (with `--variant`, if any) prints the identity a build would embed instead. That one also binds the variant and the update key.
- Every identity binds the device and profile names. `wellfed-bench` uses it to reuse a review of the same sources.

The Rust inputs are the Git-tracked and untracked, non-ignored files of:

- the SDK crates under `src/sdk/`;
- the product crates under `src/product/` they link;
- the selected firmware's crate (not `target` directories there, nor other firmwares or host tools);
- the device's `device.json` and the files it names that the profile stages;
- the builder's own `src/` and the two `wellfed-image` modules its layout audit shares.

So a changed recipe or audit is a new identity; the builder's fixture tests are not. Ignored editor or build files and the checkout's location don't change the ID, which is why `source-id` needs a Git checkout.

SDK builds freeze the input map, embed its real identity, and record clock, output, LED, routing and shared-pad policies for host verification. A compile-only check does not produce a reviewed installable image.

| Command | Does |
| --- | --- |
| `wellfed-build inputs --profile PROFILE` | Prints the current input map |
| `wellfed-build audit --build DIR` | Re-runs a finished build's audits, as JSON |
| `wellfed-build review` | Checks a build against both before freezing it ([development updates](uart-development.md)) |
| `cargo test -p wellfed-build -- --ignored` | The builder's fixture tests, in `make check-fixtures`. They fail when an ignored build no longer matches the current sources |

## Update signing

The replacement firmware installs an update only if its manifest is signed by the key the build embeds. Both commands run offline and never contact a device:

```sh
./target/release/wellfed-build keygen
./target/release/wellfed-build sign --review private/verification/<review> --version 2 \
  --output private/<name>.manifest
```

- **`keygen`** writes a new Ed25519 key to `private/ota-key.json` (mode 600), marked `"scope": "bench"`. It prints the public key that first-boot builds embed, which joins their identity. It refuses to replace an existing key.
- **`sign`** writes the version 2 manifest the device API takes (`wellfed_core::ota::offer`), naming the bench key's id and the image's kind. The version 1 format and `--format mqtt` are deleted. `--version` must exceed the feeder's floor (the highest version it confirmed), or come with a downgrade statement.
- **`downgrade --manifest M --device HEX --floor N`** is a statement letting the feeder whose identity key is `HEX` install the version 2 manifest `M` while its floor is `N`. A version at or below the floor, or any stock image, needs one.
- **`revoke --key-id HEX`** and **`add-key --public HEX`** are key statements. The first retires an update key id for good; the second fills the firmware's release slot ([OTA design](../firmware/ota-design.md#keys-and-anti-rollback)). Only a key built into the firmware signs them. `keygen` prints the bench key's id.

`wellfed-bench ota` links the same library and signs with `private/ota-key.json`.

### What `sign` refuses

`sign` signs only a directory under `private/` that `wellfed-build review` froze. It verifies it exactly as `wellfed-flash` verifies a review before installing it, and refuses:

- a directory without `review.json`;
- an `image.bin` or payload that differs from the review;
- a `build-manifest.json` that differs from its `manifest_sha256`;
- a stock review of anything but the pinned stock image;
- a build that is not the first-boot profile;
- a manifest without well-formed SDK and toolchain pins. These are the pins the reviewer checked against the lock it froze under, not the checkout's current lock;
- a manifest whose `OTA_All.bin` is not this image;
- missing startup, pad-table or mapping audits;
- an image that does not carry the reviewed startup identity.

The build ID comes from the verified manifest (for the stock image, the review's payload hash), never from the command line. An image whose build declares a startup motion policy signs only with `--attended-motion POLICY` naming that exact policy.

### The bench key

The key `keygen` makes is the bench key. It lives in a checkout's `private/` and signs development images for the bench's own units only. A key file without a `scope` field predates the field and is read as the bench key. The signer also loads an owner's key (`"scope": "owner"`), which `wellfed update-key new` writes. A feeder trusts that key only once the owner enrolls it there ([OTA design](../firmware/ota-design.md#keys-and-anti-rollback)), and it signs no key statement. No other scope loads.

A public release needs an offline release key first. That key never sits in a checkout, and this tool does not create it.

## Next steps

- [Development updates](uart-development.md): freeze a build and install it over UART.
- [Firmware guide](../../src/firmware/wellfed/README.md): write your own firmware.
- [Architecture](../firmware/README.md) and [third-party notices](../../THIRD_PARTY.md).
