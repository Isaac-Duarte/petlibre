# Development setup and checks

Linux is the initial development platform. Ordinary checks require Rust **1.99.1** through rustup, `cargo-nextest` and `cargo-deny` (`cargo install cargo-nextest cargo-deny`), GNU Make, CMake and a C compiler (the `unicorn-engine` crate builds the Unicorn emulator library; on Linux it also links libatomic), and loopback socket access. [rust-toolchain.toml](../../rust-toolchain.toml) pins rustfmt, Clippy, and the three ARM targets. SDK-backed builds have [additional prerequisites](firmware-builds.md).

## Fresh checkout

Run from the repository root:

```sh
cargo build --locked
make check
```

Every host tool is Rust; the [tools index](../../src/tools/README.md) lists what each does and which ones open a serial port. The crates live under `src/`, one folder per package; `wellfed-flash` is the [Rust UART flash client](rust-flash.md), `wellfed-bench` the [relay bench](../devices/plaf103/bench.md) (with `wellfed-relay` its relay board driver, `wellfed-itest` its integration suite and `wellfed-dashboard` its read-only page), `wellfed-audit` the offline evidence audits (startup, FW14 ADC, FW30 transients, full-flash comparison), `wellfed-build` the firmware build and the [frozen image review](uart-development.md), `wellfed-emu` the vendor factory loader and flash helper executed under Unicorn, and `wellfed-formats` the files those tools pass to each other (build manifest and identity, stock layout, SDK/toolchain locks, serial-event records, credential and key files, the bench's config, progress document, run records and suite catalog), typed once rather than read as untyped JSON in each. `cargo xtask check-repository` is the repository check.

Rustup and Cargo download dependencies on initial setup. Check targets do not install packages, fetch SDKs, contact a device, change firewall rules, or start persistent services. Network tests bind loopback sockets with synthetic identities. Environments that prohibit local sockets need permission to run those tests; skipping them is not a passing result.

## Check targets

| Target | Coverage |
| --- | --- |
| `check-rust` | Formatting, Clippy with warnings denied, workspace host tests under cargo-nextest, doctests |
| `check-deps` | cargo-deny: licenses, advisories, duplicate versions and sources ([deny.toml](../../deny.toml)), then the fuzz crate's lockfile under [its own policy](../../src/fuzz/deny.toml) |
| `check-cross` | SDK-independent KM0/KM4 FFI static libraries |
| `check-virtual-feeder` | The `wellfed-sim` worker and harness: deterministic feeding against the measured mechanism, faults, journal cuts, restarts, and the liveness invariants (also part of `check-rust`) |
| `check-docs` | Local Markdown links, repository portability checks, no Python under `src/`, and the source rules (raw output writers, card ids) |
| `check-fixtures` | Explicit optional binary/SDK evidence suite; missing inputs or skips fail |
| `check` | All contributor checks except optional fixture validation |

The server's committed contracts are checked inside `check-rust`: a test fails when `src/server/wellfed-server/openapi.json` is stale (`UPDATE_OPENAPI=1 cargo test -p wellfed-server --test openapi` rewrites it), and the store's SQL compiles only against its committed `.sqlx/` metadata. After changing a query or a migration, `make sqlx-prepare` rebuilds that metadata from a scratch database; it needs the `sqlite3` command line, and no other target does.

`check-cross` checks portable compilation, not SDK-linked hardware ports. Evidence tests are explicitly ignored in the ordinary suite and selected by `check-fixtures`; an ignored test is not passing evidence.

Generated Rust artifacts live in `target/`; the simulator worker and temporary verification outputs can create ignored `private/` subdirectories. No existing backups or evidence are required. Use [.gitignore](../../.gitignore) as the storage boundary, and keep secrets out of the tracked examples.

## Fuzzing

The network parsers also have [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) targets in [src/fuzz/](../../src/fuzz/Cargo.toml), a crate outside the workspace with its own lockfile, so `make check` never builds it. Install the runner once with `cargo install cargo-fuzz` (it also needs a C++ compiler for libFuzzer), then run one target for a bounded time:

```sh
cargo xtask fuzz api-connection --seconds 60
```

| Target | Input |
| --- | --- |
| `api-connection` | Any bytes on a new device API connection: the clear opening frame, the IK, invite and recovery handshakes, and a full session table |
| `api-session` | Requests sealed inside the owner's open session, each answer opened and decoded |
| `proto-decode` | Every device protocol and setup message type, decoded and re-encoded |
| `noise-responder` | The first handshake message of each Noise pattern, into any payload buffer |
| `setup` | BLE setup messages to the feeder's responder, and a genuine client's request, code proof and Wi-Fi |
| `ota-manifest` | The version 2 update manifest an offer carries, and chunks to the bank-1 writer |

`--seconds` defaults to 60. The command runs the pinned toolchain with `RUSTC_BOOTSTRAP=1` for libFuzzer's unstable compiler flags, so no nightly toolchain is needed. Builds, grown corpora and crash inputs stay in ignored `src/fuzz/target/`, `src/fuzz/corpus/` and `src/fuzz/artifacts/`; on disk a target is named with underscores (`api_connection`), as `src/fuzz/Cargo.toml` lists it. A crash is a bug in the parser: replay it with `cargo xtask fuzz <target> --replay src/fuzz/artifacts/<target_name>/<file>`, fix it, and add the input as a regression test beside the property that should have caught it. The device API and setup targets share their drivers with the property tests ([support](../../src/product/wellfed-feeder/tests/support/mod.rs)).

## Work on a component

```sh
cargo fmt --all
make check-rust
make -C src/firmware/vendor test
make check-virtual-feeder
```

The simulator connects the real Rust owner/controller/ports to fake SDK inputs and NOR storage, under the PLAF103 firmware's feeding limits (`wellfed_sim::plaf103`, which the firmware's tests hold equal to `feeding::CONFIG`). Its plant (`wellfed_sim::plant`) is the mechanism the FW16 feeds measured ([motor pulse](../firmware/motor-pulse.md)): PA18 rests HIGH and reads LOW for about 1.19 s at each mark, a portion's rise about 9.07 s of drive after the last, the start-up surge one poll over the current limit, and the motor current while driving; a scenario's `plant` object changes the seed, the jitter, the rates of missed and bouncing edges, or the period (with food, about 9.8 s). The rotor keeps its position across stops, power cuts and restarts. The worker also stands in for the loop's claim on the outputs and for a background store writer that takes the write turn and a real flash lease (`write`).

It covers command IDs, sensor faults, blocked outlet, missing and bouncing feedback, watchdog stalls, interrupted journal writes, and two liveness invariants: after a power cut anywhere (each journal mutation of a reservation, result or resolution, and moments through a feed), a new feed ID is accepted and completes; and a feed completes, and every store write lands, with writes before, during and after it. Simulation evidence, not hardware reliability: it does not model all electrical behavior, and a double bowl's mechanism is unmeasured (its scenarios set their own marks). Example scenarios are in [src/firmware/vendor/sim/scenarios](../../src/firmware/vendor/sim/scenarios); run one with `cargo run -p wellfed-sim --bin virtual-feeder -- --scenario FILE --report NEW-FILE`.

The repository is self-contained: SDK/toolchain copies and fixtures resolve locally. Original research documents/tools/history are retained in ignored `private/research-archive/`; see [provenance](provenance.md). Historical evidence paths are records, not instructions to access another checkout.

The rules the code follows are in the [Rust standard](rust.md) and the [TypeScript standard](typescript.md). See [local server](local-server.md), [SDK builds](firmware-builds.md), [optional fixtures](fixtures.md), and [contribution guidance](../../CONTRIBUTING.md).
