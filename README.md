# Wellfed

Local control and experimental replacement firmware for the **PETLIBRO Granary PLAF103** pet feeder. Independent community research; no vendor affiliation.

Wellfed contains a Rust local server for stock firmware, a Rust replacement-firmware workspace, and documented hardware/firmware findings. You can build, test, and simulate the code without a feeder, vendor binaries, or private research files.

## Current capabilities

| Component | Established | Remaining work |
| --- | --- | --- |
| Stock-firmware local server | Local MQTT connection and supervised dispensing demonstrated on stock 1.1.15; authenticated API and durable request tracking | Measured portions, sustained offline operation, cold boots, and reconnect reliability |
| Replacement firmware | Feeding, sensor, watchdog, journal, and flash-coordination components tested on host models; Rust diagnostic booted on one PLAF103; build-bound reports, stopped GPIO startup, and bounded LED control added | Functional physical feeding, networking, schedules, panel/audio behavior, real flash persistence, and hardware acceptance |
| UART and recovery | UART access, matching full 8 MiB backups, independent loader entry, bounded write-back, stock restoration/boot, diagnostic installation, and faster development updates demonstrated on the research unit | General device compatibility and power-cut/full-bank recovery acceptance |

The replacement is experimental. A diagnostic PASS does not establish working physical feeding. See [firmware status and architecture](docs/firmware/README.md) and [recovery limits](docs/hardware/recovery.md).

## Start here

- [Resume the current firmware work](RESUME.md): installed image, last bench state, evidence, and next task
- [Hardware identification and signal map](docs/hardware/README.md)
- [UART wiring and loader entry](docs/hardware/uart.md)
- [Stock firmware and replacement architecture](docs/firmware/README.md)
- [Development setup and checks](docs/development/README.md)
- [Local server setup and API](docs/development/local-server.md)
- [SDK firmware builds](docs/development/firmware-builds.md)
- [Firmware feature board](FIRMWARE_BOARD.md) and [startup/indicator diagnostics](docs/firmware/startup-diagnostics.md)
- [UART development updates](docs/development/uart-development.md)

## Build and test

Install Rust through rustup, GNU Make, CMake and a C compiler (for the Unicorn emulator the host tools link). The pinned Rust toolchain includes rustfmt, Clippy, and the ARM targets. On Linux:

```sh
cargo build --locked
make check
```

Initial setup downloads dependencies. Checks use host models and loopback networking; they do not connect to a feeder, open serial ports, or modify firewall rules. SDK downloads and hardware operations are separate, explicit workflows.

## Repository layout

| Location | Contents |
| --- | --- |
| `src/sdk/` | The Wellfed SDK that firmwares build on: `wellfed-sdk` (the runtime every firmware runs inside, and the firmware API), `wellfed-core` (hardware-independent feeding, journal and flash logic), `wellfed-hal` and `wellfed-hal-common` (AmebaD ports), `wellfed-amebad-sys` (pinned vendor SDK bindings and the host fake), `wellfed-sdk-c` (the C ABI and per-core archives), `wellfed-testkit` and `wellfed-sim` (the virtual feeder) |
| `src/tools/` | Host tools: `wellfed-local` (stock-firmware local server), `wellfed-flash` (UART flash reads, updates and recovery), `wellfed-bench` (relay bench and its progress page, built in with `--features ui`), `wellfed-audit` (offline evidence audits), `wellfed-build` (SDK firmware builds, their link audits and the frozen image review), `wellfed-emu` (the vendor factory loader and flash helper, executed under Unicorn), `wellfed-formats` (the file formats those host tools share), `xtask` (`cargo xtask check-repository`) |
| `src/firmware/wellfed/` | One crate per firmware built on the SDK ([guide](src/firmware/wellfed/README.md)): `reference` (the diagnostic application and its bench variants) and `blink` (the minimal template) |
| `src/firmware/vendor/` | The vendor SDK side: C entry hooks and ports, the pinned SDK and toolchain locks, the stock layout, simulator scenarios, and the image build (`make -C src/firmware/vendor`) |
| `src/web/` | Optional web frontends (pnpm workspace): the bench page, shared design tokens and components, and API types generated from Rust. `make ui` builds them |
| `src/scripts/` | Developer scripts: `dev-dashboard` serves the bench page for this checkout |
| `docs/` | Hardware findings, firmware contracts, and contributor workflows |
| `private/` | Ignored contributor-owned secrets, evidence, dependencies, and artifacts |

The existing `wellfed-*` crate names and `wellfed-local` executable are retained for compatibility. [Contributions](CONTRIBUTING.md) are welcome. First-party material is [MIT licensed](LICENSE); see [third-party provenance](THIRD_PARTY.md). This repository contains no vendor firmware or device backups.
