# Development setup and checks

Linux is the initial development platform. Ordinary checks require Rust **1.99.1** through rustup, GNU Make, CMake and a C compiler (the `unicorn-engine` crate builds the Unicorn emulator library; on Linux it also links libatomic), and loopback socket access. [rust-toolchain.toml](../../rust-toolchain.toml) pins rustfmt, Clippy, and the three ARM targets. SDK-backed builds have [additional prerequisites](firmware-builds.md).

## Fresh checkout

Run from the repository root:

```sh
cargo build --locked
make check
```

Every host tool is Rust; the [tools index](../../src/tools/README.md) lists what each does and which ones open a serial port. The crates live under `src/`, one folder per package; `wellfed-flash` is the [Rust UART flash client](rust-flash.md), `wellfed-bench` the [relay bench](../hardware/bench.md), `wellfed-audit` the offline evidence audits (startup, FW14 ADC, FW30 transients, full-flash comparison), `wellfed-build` the firmware build and the [frozen image review](uart-development.md), `wellfed-emu` the vendor factory loader and flash helper executed under Unicorn, and `wellfed-formats` the files those tools pass to each other (build manifest and identity, stock layout, SDK/toolchain locks, serial-event records, credential and key files), typed once rather than read as untyped JSON in each. `cargo xtask check-repository` is the repository check.

Rustup and Cargo download dependencies on initial setup. Check targets do not install packages, fetch SDKs, contact a device, change firewall rules, or start persistent services. Network tests bind loopback sockets with synthetic identities. Environments that prohibit local sockets need permission to run those tests; skipping them is not a passing result.

## Check targets

| Target | Coverage |
| --- | --- |
| `check-rust` | Formatting, Clippy with warnings denied, workspace host tests |
| `check-cross` | SDK-independent KM0/KM4 FFI static libraries |
| `check-virtual-feeder` | The `wellfed-sim` worker and harness: deterministic feeding, faults, journal cuts, and restart scenarios (also part of `check-rust`) |
| `check-docs` | Local Markdown links, repository portability checks, no Python under `src/`, and the FW08 transport isolation rule |
| `check-fixtures` | Explicit optional binary/SDK evidence suite; missing inputs or skips fail |
| `check` | All contributor checks except optional fixture validation |

`check-cross` checks portable compilation, not SDK-linked hardware ports. Evidence tests are explicitly ignored in the ordinary suite and selected by `check-fixtures`; an ignored test is not passing evidence.

Generated Rust artifacts live in `target/`; the simulator worker and temporary verification outputs can create ignored `private/` subdirectories. No existing backups or evidence are required. Use [.gitignore](../../.gitignore) as the storage boundary, and keep secrets out of the tracked examples.

## Work on a component

```sh
cargo fmt --all
make check-rust
make -C src/firmware/vendor test
make check-virtual-feeder
```

The simulator connects the real Rust owner/controller/ports to fake SDK inputs and NOR storage. It covers command IDs, sensor faults, blocked outlet, missing feedback, watchdog stalls, and interrupted journal writes. It does not model all electrical behavior. Example scenarios are in [src/firmware/vendor/sim/scenarios](../../src/firmware/vendor/sim/scenarios); run one with `cargo run -p wellfed-sim --bin virtual-feeder -- --scenario FILE --report NEW-FILE`.

The repository is self-contained: SDK/toolchain copies and fixtures resolve locally. Original research documents/tools/history are retained in ignored `private/research-archive/`; see [provenance](provenance.md). Historical evidence paths are records, not instructions to access another checkout.

See [local server](local-server.md), [SDK builds](firmware-builds.md), [optional fixtures](fixtures.md), and [contribution guidance](../../CONTRIBUTING.md).
