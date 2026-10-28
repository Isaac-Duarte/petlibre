# Optional fixtures

Some tests check the code against real binaries: the stock image, the SDK, and the research unit's recordings. This page says what those inputs are and where they go. Ordinary contributor checks need none of them.

## Run the suite

```sh
make check-fixtures
```

The suite fails on a missing fixture, an incompatible hash, an error or a skip. It downloads nothing and produces no missing evidence. A passing synthetic suite says nothing about these checks.

Evidence tests are Rust tests marked `#[ignore]`, so plain `cargo test` lists them as ignored, not passed. Use the Make target when you report results.

The emulation, review and flash-client evidence tests read their inputs from the checkout they were built in. Those are `wellfed-emu`, `wellfed-build`'s `tests/review.rs` and `wellfed-flash`'s `tests/emulated.rs`. A Git worktree has no evidence, so set `WELLFED_FIXTURE_ROOT` to another checkout to use its `private/`. Their temporary outputs then go under that checkout's `private/` and are removed afterwards.

The update-signing tests of `wellfed-build` and `wellfed-bench` freeze the pinned stock OTA (`private/firmware/PLAF103_OTA_010115.bin`). They sign it as `wellfed-bench ota` would, since only that exact image may be signed as stock.

## Where inputs go

| Location | What it holds |
| --- | --- |
| `private/firmware/PLAF103_OTA_010115.bin` | The stock 1.1.15 OTA you obtained yourself. The layout file records its exact hash |
| `private/upstream/ameba-rtos-d/` | The SDK checkout from an explicit bootstrap. It supplies candidate ROM and helper inputs |
| `private/toolchains/`, `private/firmware-build/` | SDK build artifacts and audited tools. A build whose manifest inputs differ from the current sources is stale, and its tests fail until you rebuild |
| `private/fixtures/baseline.bin` | The exact historical 8 MiB research baseline the pinned recovery checks require |
| `private/fixtures/current-read/flash-read.bin` | A recorded current full snapshot. The first-boot mapping test places the build into it |
| `private/fixtures/rom-inspection/report.json` | A finalized ROM report. Its linked serial evidence must stay beside it |
| `private/fixtures/development-source/`, `development-target/` | Intact frozen review bundles for the helper-model and CLI tests |
| `private/fixtures/development-bounded-read/report.json` | The bounded-read evidence that goes with them |

Keep each capture or report bundle intact, and pass explicit report arguments when a test asks. Two independently captured bundles stay in separate subdirectories, each with its companion files. A directory alias must keep the companion serial and read files; aliasing one report file is not enough.

If you import your own private archive, local directory or file symlinks can connect these locations to unchanged original bundles. They stay ignored. The public checkout has no such aliases, and default tests don't depend on fixtures being present. Existing SDK artifacts may not validate against changed Wellfed inputs: rebuild and freeze a new target instead of rewriting old evidence.

## What these paths don't prove

These are test conventions, not proof of a valid session. They don't make the research unit's exact recovery profile portable. Nobody else is expected to have its baseline or its historical artifact.

Get stock updates from your own authorized device and app, read original backups from your own hardware, and get the SDK and toolchain through the pinned upstream downloads. Those inputs support your own analysis. A new device's backup will intentionally fail a writer pinned to the original research profile.

The historical baseline, captures and exact accepted diagnostic are not publicly distributed. So the full reference suite can't be reproduced from public files alone. For fixture-independent validation, run `make check`.

> [!WARNING]
> Never remove provenance checks or substitute hashes to get a green optional suite. To support another unit, design and review per-device compatibility and preservation criteria as separate work.
