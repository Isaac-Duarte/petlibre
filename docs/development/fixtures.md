# Optional binary and SDK fixtures

Ordinary contributor checks need no private binary inputs. The optional suite is selected explicitly:

```sh
make check-fixtures
```

It fails for missing fixtures, incompatible hashes, errors, or skips. It does not download binaries or produce missing evidence. A passing synthetic suite does not imply these checks passed. Evidence tests are Rust tests marked `#[ignore]`, so plain `cargo test` lists them as ignored, not passed; use the Make target when reporting contributor results.

The emulation, review and flash-client evidence tests (`wellfed-emu`, `wellfed-build`'s `tests/review.rs`, `wellfed-flash`'s `tests/emulated.rs`) read their inputs from the checkout they were built in. Set `WELLFED_FIXTURE_ROOT` to another checkout to use its `private/` instead, as a Git worktree (which has no evidence) must; their temporary outputs then go under that checkout's `private/` and are removed afterwards.

The update-signing tests of `wellfed-build` and `wellfed-bench` freeze the pinned stock OTA (`private/firmware/PLAF103_OTA_010115.bin`) and sign it as `wellfed-bench ota` would, since only that exact image may be signed as stock.

## Generic local locations

| Input/location | Purpose and availability |
| --- | --- |
| `private/firmware/PLAF103_OTA_010115.bin` | Contributor-obtained stock 1.1.15 OTA; exact hash is recorded in the layout file |
| `private/upstream/ameba-rtos-d/` | SDK checkout obtained by explicit bootstrap; supplies candidate ROM/helper inputs |
| `private/toolchains/` and `private/firmware-build/` | Explicit SDK build artifacts and audited tools; a build whose manifest inputs differ from the current sources is stale and its tests fail until rebuilt |
| `private/fixtures/baseline.bin` | Exact historical 8 MiB research baseline required by pinned recovery checks |
| `private/fixtures/current-read/flash-read.bin` | Recorded current full snapshot the first-boot mapping test places the build into |
| `private/fixtures/rom-inspection/report.json` | Finalized ROM report; linked serial evidence must remain beside it |
| `private/firmware/experiments/` | Explicitly prepared stock-version OTA pairs that `wellfed-local`'s pinned-image test reads |

The generic locations replace dated workstation paths. They do **not** make the exact research-unit recovery profile portable. No other contributor is expected to possess its baseline or historical artifact. Keep each capture/report bundle intact and supply explicit report arguments when required; two independently captured bundles should remain in separate subdirectories with their companion files. Generic fixture paths are test conventions, not proof of valid session provenance.

Obtain stock updates from your own authorized device/app workflow, read original backups from your own hardware, and get the SDK/toolchain through the pinned upstream downloads. Those inputs may support independent analysis, but a new device's backup will intentionally fail a writer pinned to the original research profile. The historical baseline, captures, and exact accepted diagnostic are not publicly distributed, and the complete reference suite is not reproducible from public files alone.

For useful fixture-independent validation, run `make check`. To extend recovery support, design and review explicit per-device compatibility and preservation criteria as separate work; do not remove provenance checks or substitute hashes in order to get a green optional suite.

## Development-update fixtures

The new helper-model and CLI tests additionally use `private/fixtures/development-source/` and `development-target/` for intact frozen review bundles, and `private/fixtures/development-bounded-read/report.json` for the associated bounded-read evidence. Generic directory aliases must preserve companion serial/read files; individual report-file aliases are insufficient.

If importing your own private archive, local directory/file symlinks can connect these conventions to unchanged original evidence bundles. They remain ignored. The public checkout has no such aliases, and default tests stay independent of fixture presence. Existing SDK artifacts may not validate against changed Wellfed inputs; rebuild and freeze a new target instead of rewriting historical evidence.
