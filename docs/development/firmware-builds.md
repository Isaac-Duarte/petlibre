# SDK-backed firmware builds

Host tests and cross-compilation checks need no vendor SDK. Full images use the pinned Realtek AmebaD SDK and ASDK compiler specified by [sdk.lock.json](../../firmware/sdk.lock.json). The SDK baseline is a family-level reference, not a source match for stock `sdk-amebad_v6.2C-RC`.

## Prerequisites and explicit setup

The existing wrapper supports **Linux x86_64**. It needs Git, Make, Bash, tar, standard GNU tools, Python 3.10.12+ with tar's data extraction filter, the pinned Rust toolchain, libclang for bindgen, and the 32-bit loader/runtime needed by Realtek's i686 compiler. Install platform prerequisites yourself; the wrapper installs no system packages.

```sh
make -C firmware bootstrap
make -C firmware baseline
make -C firmware bringup
```

`bootstrap` explicitly fetches the SDK revision and downloads/checksums the compiler archive. The archive digest was computed from the official release download; it is not an independently supplied upstream signature. Existing unexpected SDK revisions are refused. Check targets never bootstrap implicitly.

## Profiles

| Profile | Behavior and inputs |
| --- | --- |
| `baseline` | Unchanged SDK example, for build comparison |
| `bringup` | SDK example with Rust diagnostic, STOP/off initialization, and bounded indicator entry hook |
| `plaf103` | Diagnostic with stock-derived bank addresses and audited KM0 pin pulls; requires the verified stock OTA |
| `plaf103-firstboot` | Minimal UART-only SDK mains plus Rust synthetic diagnostic; same stock-reference requirements |

```sh
make -C firmware plaf103
make -C firmware plaf103-firstboot
```

Those two profiles require a contributor-supplied exact stock reference at `private/firmware/PLAF103_OTA_010115.bin`; see [fixtures](fixtures.md). Builds proceed KM0 before KM4 and retain input hashes, artifact hashes, logs, and successful manifests. Bindgen gets SDK/newlib paths from the wrapper through `PETLIBRO_SDK_DIR` and `PETLIBRO_NEWLIB_INCLUDE`.

| Ignored location | Contents |
| --- | --- |
| `private/upstream/ameba-rtos-d/` | Pinned upstream checkout |
| `private/toolchains/` | Downloaded archive and extracted compiler |
| `private/firmware-build/<profile>/` | Generated SDK worktree, Rust archives, logs, and manifest |

Author changes in tracked source, not generated SDK worktrees. Profiles stage tracked files and apply audited overlays. Vendor timestamps mean output is not guaranteed byte-for-byte reproducible. A rebuilt UART diagnostic will generally differ from the exact artifact accepted by the retained writer; do not update its hash to bypass review.

Builds package application images and audit symbols/layout/pin pulls/flash hooks where applicable. They do not flash, actuate a motor, validate a physical board, or authorize replacing the factory loader. Upstream example startup can still touch hardware before the diagnostic. See [architecture](../firmware/README.md), [third-party notices](../../THIRD_PARTY.md), and [recovery restrictions](../hardware/recovery.md).

## Source identity and physical startup behavior

`python3 firmware/tools/build.py source-id` computes the current firstboot-profile source ID offline without bootstrapping the SDK. `check-cross` supplies that ID to the SDK-independent KM0 port compile. A compile-only check does not produce a reviewed installable image. SDK builds freeze the input map, embed its real identity, and record clock/output/LED/routing policies for host verification.

The diagnostic now configures STOP/off GPIO, restores shared-pad supply startup in the minimal KM0 main, and pulses the four indicators. An unopened feeding backend no longer means no physical GPIO changes. See [startup behavior and outstanding readback faults](../firmware/startup-diagnostics.md). Build commands themselves still do not access hardware. Use [development image review](uart-development.md) for a source-bound target; copied historical manifests may be stale after porting.
