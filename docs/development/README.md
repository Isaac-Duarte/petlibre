# Getting started

This guide takes you from a fresh checkout to a passing `make check`. It then shows which tool does what, how to work on one component, and how to run the simulator and the fuzzers. Nothing here needs a feeder, and nothing here touches one.

These docs follow the [writing guide](writing.md). The code follows the [Rust standard](rust.md) and the [TypeScript standard](typescript.md).

## Prerequisites

We develop on Linux. You need:

- Rust **1.99.1** through rustup. [rust-toolchain.toml](../../rust-toolchain.toml) pins it, with rustfmt, Clippy and the three ARM targets, so rustup fetches them on first use.
- `cargo-nextest` and `cargo-deny`: `cargo install cargo-nextest cargo-deny`.
- GNU Make, CMake and a C compiler. The `unicorn-engine` crate builds the Unicorn emulator library with them. On Linux it also links libatomic.
- Permission to open loopback sockets. Network tests bind them with synthetic identities.

Full firmware images need more: the pinned vendor SDK and its compiler. See [firmware builds](firmware-builds.md).

## Set up

1. Clone the repository and open its root.

2. Run `cargo build --locked`.

   Rustup and Cargo download the toolchain and dependencies the first time. You should see the build finish with no errors.

3. Run `make check`.

   You should see formatting, Clippy, the tests, the cross-builds, the simulator and the repository checks all pass. The first run of `check-deps` downloads the RustSec advisory database.

If your environment forbids local sockets, give the network tests permission to run. Skipping them is not a passing result.

## What checks do and don't touch

The check targets never install packages, fetch an SDK, contact a device, change firewall rules or start a persistent service. Network tests use loopback only.

Build output goes to `target/`. The simulator worker and temporary verification outputs may create ignored folders under `private/`. You don't need any existing backup or evidence. [.gitignore](../../.gitignore) is the storage boundary: keep secrets out of the tracked examples.

The repository is self-contained. SDK and toolchain copies and fixtures resolve locally. The original research documents, tools and history sit in the ignored `private/research-archive/`; see [provenance](provenance.md). An evidence path recorded in an old file is a record, not an instruction to open another checkout.

## Check targets

| Target | What it covers |
| --- | --- |
| `check-rust` | Formatting, Clippy with warnings denied, workspace host tests under cargo-nextest, doctests |
| `check-deps` | cargo-deny: licenses, advisories, duplicate versions and sources ([deny.toml](../../deny.toml)), then the fuzz crate's and the bench board firmware's lockfiles under their own policies ([fuzz](../../src/fuzz/deny.toml), [bench board](../../src/firmware/bench/wellfed-bench-board/deny.toml)) |
| `check-cross` | The SDK-independent KM0 and KM4 FFI static libraries, and `check-bench-board`: Clippy and a release build of the [bench board firmware](../../src/firmware/bench/README.md) for the RP2040 |
| `check-virtual-feeder` | The simulator; see [the simulator](#the-simulator). Also part of `check-rust` |
| `check-docs` | Local Markdown links, portability checks, no Python under `src/`, and the source rules (raw output writers, card ids) |
| `check-fixtures` | The optional binary and SDK evidence suite. Missing inputs or skips fail ([fixtures](fixtures.md)) |
| `check-web` | The web frontends: ESLint, Prettier, licenses, types, unit tests, the build and Playwright. Needs Node and pnpm ([TypeScript standard](typescript.md)) |
| `check` | Every contributor check except `check-fixtures` and `check-web` |

Notes:

- `check-cross` proves portable compilation. It does not build SDK-linked hardware ports.
- Evidence tests are `#[ignore]`d in the ordinary suite, and only `check-fixtures` selects them. An ignored test is not passing evidence.
- `check-rust` also checks the server's committed contracts. A test fails when `src/server/wellfed-server/openapi.json` is stale; `UPDATE_OPENAPI=1 cargo test -p wellfed-server --test openapi` rewrites it.
- The store's SQL compiles only against its committed `.sqlx/` metadata. After you change a query or a migration, run `make sqlx-prepare` to rebuild it from a scratch database. That target needs the `sqlite3` command line; no other target does.

## Which tool do I use?

Every host tool is Rust. The full inventory, with which tools touch hardware, is in [src/tools/README.md](../../src/tools/README.md).

| I want to… | Use | Read |
| --- | --- | --- |
| Pair with a feeder, feed, plans, settings, history and status | `wellfed` | [commands](../../src/tools/README.md#the-wellfed-command-line), [device API](../firmware/device-api.md) |
| Run against a feeder without hardware | `wellfed-feeder-sim` | [tools](../../src/tools/README.md) |
| Build a firmware image | `wellfed-build` | [firmware builds](firmware-builds.md) |
| Freeze a build for installing, or sign it as an update | `wellfed-build review`, `sign` | [development updates](uart-development.md) |
| Read or write the feeder's flash over UART | `wellfed-flash` | [Rust flash client](rust-flash.md) |
| Drive the relay bench: power, loader, OTA, the suite | `wellfed-bench` | [relay bench](../devices/plaf103/bench.md) |
| Serve stock 1.1.15 for an install | `wellfed-stock` | [stock server](stock-server.md) |
| Audit a capture or compare full-flash backups | `wellfed-audit` | [tools](../../src/tools/README.md) |
| Test feeding logic against a modeled mechanism | `wellfed-sim` | [the simulator](#the-simulator) |
| Run the repository checks or a fuzzer | `cargo xtask` | [fuzzing](#fuzzing) |

The files these tools pass to each other are typed once, in `wellfed-formats`.

## Work on a component

Run these from the repository root after an edit:

```sh
cargo fmt --all
make check-rust
make -C src/firmware/vendor test
make check-virtual-feeder
```

Then run `make check` before you hand the change over. The [contribution guidance](../../CONTRIBUTING.md) covers the rest.

## The simulator

The simulator runs the real Rust feeding owner, controller and ports against fake SDK inputs and fake NOR storage. It uses the PLAF103 firmware's feeding limits (`wellfed_sim::plaf103`), and the firmware's tests hold those equal to `feeding::CONFIG`. No feeder needed.

### The modeled mechanism

The plant (`wellfed_sim::plant`) is the mechanism measured on real feeds ([motor pulse](../firmware/motor-pulse.md)):

- The mark input, PA18, rests HIGH and reads LOW for about 1.19 s at each mark.
- The next portion's mark arrives about 9.07 s of drive after the last one. With food in the hopper the period is about 9.8 s.
- The start-up surge lasts one poll over the current limit.
- While driving, the motor current has a mean near 60 (scaled), with peaks to about 167, as the empty feeds read.
- The rotor keeps its position across stops, power cuts and restarts.

A scenario's `plant` object can change the seed, the jitter, the rates of missed and bouncing edges, or the period.

A `wedge` stalls the rotor going forward at the plant's stall current. That current is unmeasured. A wedge is either freed by the first reversal or not ([jams](../firmware/jams-and-sensor-faults.md)).

The worker also stands in for the loop's claim on the outputs. It plays a background store writer too, which takes the write turn and a real flash lease (`write`).

### What it checks

Scenarios cover command IDs, sensor faults, a blocked outlet, missing and bouncing feedback, watchdog stalls and interrupted journal writes. Two liveness invariants hold across them:

- After a power cut anywhere, a new feed ID is accepted and completes. "Anywhere" means each journal mutation of a reservation, result or resolution, and moments through a feed.
- A feed completes, and every store write lands, with writes before, during and after it.

> [!NOTE]
> This is simulation evidence, not hardware reliability. It does not model all electrical behavior. A double bowl's mechanism is unmeasured, so its scenarios set their own marks.

### Run a scenario

Example scenarios are in [src/firmware/vendor/sim/scenarios](../../src/firmware/vendor/sim/scenarios).

```sh
cargo run -p wellfed-sim --bin virtual-feeder -- --scenario FILE --report NEW-FILE
```

You should see the report written to `NEW-FILE`. `make check-virtual-feeder` runs the whole set.

## Fuzzing

The network parsers have [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) targets in [src/fuzz/](../../src/fuzz/Cargo.toml). That crate sits outside the workspace with its own lockfile, so `make check` never builds it.

1. Run `cargo install cargo-fuzz` once. libFuzzer also needs a C++ compiler.

2. Run one target for a bounded time:

   ```sh
   cargo xtask fuzz api-connection --seconds 60
   ```

   `--seconds` defaults to 60. The command uses the pinned toolchain with `RUSTC_BOOTSTRAP=1` for libFuzzer's unstable compiler flags, so you don't need a nightly toolchain.

| Target | Input |
| --- | --- |
| `api-connection` | Any bytes on a new device API connection: the clear opening frame, the IK, invite and recovery handshakes, and a full session table |
| `api-session` | Requests sealed inside the owner's open session, each answer opened and decoded |
| `proto-decode` | Every device protocol and setup message type, decoded and re-encoded |
| `noise-responder` | The first handshake message of each Noise pattern, into any payload buffer |
| `setup` | BLE setup messages to the feeder's responder, and a genuine client's request, code proof and Wi-Fi |
| `ota-manifest` | The version 2 update manifest an offer carries, and chunks to the bank-1 writer |

Builds, grown corpora and crash inputs stay in the ignored `src/fuzz/target/`, `src/fuzz/corpus/` and `src/fuzz/artifacts/`. On disk a target's name uses underscores (`api_connection`), as `src/fuzz/Cargo.toml` lists it.

A crash is a bug in the parser. To handle one:

1. Replay it with `cargo xtask fuzz <target> --replay src/fuzz/artifacts/<target_name>/<file>`.
2. Fix the parser.
3. Add the input as a regression test beside the property that should have caught it.

The device API and setup targets share their drivers with the property tests ([support](../../src/product/wellfed-device-api/tests/support/mod.rs)).

## Next steps

- [Firmware builds](firmware-builds.md): build a full image with the vendor SDK.
- [Optional fixtures](fixtures.md): run the evidence suite.
- [Stock server](stock-server.md): the server stock installs from.
- [Roadmap](../roadmap.md): what works today.
- [Glossary](../glossary.md): the terms these pages use.
